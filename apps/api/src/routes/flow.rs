//! HTTP handlers for the Flow REST endpoints this package ships.
//!
//! `rest-api-v1.md` "v0.4 Flow Alpha", minus `collab`/`collab/verify`/the WebSocket ticket pair,
//! which live in `routes::collab` — see `apps/api/src/flow/mod.rs`'s module docs.
//!
//! ```text
//! POST /api/v1/workspaces/{workspace_id}/flow/objects
//! GET  /api/v1/workspaces/{workspace_id}/flow/objects
//! GET  /api/v1/flow/objects/{object_id}
//! POST /api/v1/flow/objects/{object_id}/commands
//! GET  /api/v1/flow/objects/{object_id}/relations
//! GET  /api/v1/flow/objects/{object_id}/bootstrap
//! GET  /api/v1/flow/objects/{object_id}/grants
//! PUT  /api/v1/flow/objects/{object_id}/grants
//! PUT  /api/v1/flow/objects/{object_id}/inheritance
//! GET  /api/v1/flow/objects/{object_id}/history
//! GET  /api/v1/flow/objects/{object_id}/diff
//! GET  /api/v1/workspaces/{workspace_id}/flow/projection-lag
//! GET  /api/v1/workspaces/{workspace_id}/flow/search
//! GET  /api/v1/workspaces/{workspace_id}/features/flow
//! PUT  /api/v1/workspaces/{workspace_id}/features/flow
//! ```
//!
//! Every handler here only parses/extracts HTTP-shaped input and calls into `crate::flow`; domain
//! rules (idempotency, workspace/project/parent validation, the CRDT document lifecycle) live
//! there, not here — matching the split every other route module in this crate already uses
//! between the axum handler and its backing service/repository code.

use axum::{
    Extension, Json,
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use crate::{
    error::ApiError,
    flow::{
        collections::{self, RecordQueryPayload},
        command::{CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput},
        event_origin::{CommandOrigin, EventSource, EventSurface},
        grants::{self, Caller, GrantRequest, SetGrantsInput, SetInheritanceInput},
        policy, query,
        query::Render,
        relations, search,
    },
    response::ApiResponse,
};

#[derive(Debug, Deserialize)]
pub struct PackageExportRequest {
    pub format: String,
    #[serde(default)]
    pub include_history: bool,
    pub project_id: Option<Uuid>,
    pub at_seq: Option<i64>,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize)]
pub struct PackageArtifactSource {
    pub kind: String,
    pub package_base64: Option<String>,
    pub package_sha256: Option<String>,
    pub object_key: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PackageArtifactRequest {
    pub source: PackageArtifactSource,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize)]
pub struct PackageImportPreviewRequest {
    pub artifact_id: Uuid,
    #[serde(default)]
    pub project_mapping: std::collections::BTreeMap<Uuid, Option<Uuid>>,
    pub external_reference_policy: crate::flow::package_import::ExternalReferencePolicy,
    pub conflict_policy: crate::flow::package_import::ConflictPolicy,
    #[serde(default)]
    pub include_history: bool,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize)]
pub struct PackageImportCommitRequest {
    pub package_sha256: String,
    pub mapping_hash: String,
    pub conflict_policy: crate::flow::package_import::ConflictPolicy,
    pub confirm: bool,
    pub idempotency_key: String,
}

fn package_import_principal(id: Uuid, role: String, is_bot: bool) -> crate::flow::package_import::ImportPrincipal {
    crate::flow::package_import::ImportPrincipal {
        id,
        kind: if is_bot { "bot" } else { "user" }.to_string(),
        role,
    }
}

fn package_export_principal(id: Uuid, role: String, is_bot: bool) -> crate::flow::export::ExportPrincipal {
    crate::flow::export::ExportPrincipal {
        id,
        kind: if is_bot { "bot" } else { "user" }.to_string(),
        workspace_export_capability: is_bot && role == "admin",
        role,
    }
}

fn package_source_head() -> Result<String, ApiError> {
    let head = env!("OPENPR_EMBEDDED_GIT_COMMIT");
    if head.len() != 40
        || !head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ApiError::Internal);
    }
    Ok(head.to_string())
}

fn require_package_tool(bot: Option<&Extension<BotAuthContext>>, expected: &str) -> Result<(), ApiError> {
    if let Some(Extension(context)) = bot
        && context.tool_name.as_deref().is_some_and(|tool| tool != expected)
    {
        return Err(ApiError::Forbidden(format!(
            "package operation requires the exact {expected} tool policy"
        )));
    }
    Ok(())
}

async fn package_workspace_principal(
    state: &AppState,
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    workspace_id: Uuid,
) -> Result<(crate::flow::package_import::ImportPrincipal, axum::http::Extensions), ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, role, is_bot) =
        policy::require_flow_workspace_admin_access(state, &extensions, workspace_id).await?;
    Ok((package_import_principal(actor_id, role, is_bot), extensions))
}

pub async fn post_flow_object_export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(req): Json<PackageExportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_package_tool(bot.as_ref(), "objects.export")?;
    if req.project_id.is_some() {
        return Err(ApiError::invalid_update("object export does not accept project_id"));
    }
    let extensions = build_auth_extensions(claims, bot);
    let (_, caller) = authorization_caller(&state, &extensions, object_id).await?;
    let receipt = crate::flow::export::create_package_export(
        &state.db,
        &crate::flow::export::CreateExportRequest {
            scope: crate::flow::export::ExportScope::Object(object_id),
            format: req.format,
            at_seq: req.at_seq,
            include_history: req.include_history,
            idempotency_key: req.idempotency_key,
            source_head: package_source_head()?,
            principal: package_export_principal(caller.actor_id, caller.role, caller.principal_kind == "bot"),
        },
    )
    .await?;
    Ok(ApiResponse::success(receipt))
}

pub async fn post_flow_workspace_export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PackageExportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_package_tool(bot.as_ref(), "objects.export_workspace")?;
    if req.format != "package" || req.at_seq.is_some() {
        return Err(ApiError::invalid_update(
            "workspace export requires format=package at the accepted head",
        ));
    }
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, role, is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;
    let receipt = crate::flow::export::create_package_export(
        &state.db,
        &crate::flow::export::CreateExportRequest {
            scope: crate::flow::export::ExportScope::Workspace {
                workspace_id,
                project_id: req.project_id,
            },
            format: req.format,
            at_seq: req.at_seq,
            include_history: req.include_history,
            idempotency_key: req.idempotency_key,
            source_head: package_source_head()?,
            principal: package_export_principal(actor_id, role, is_bot),
        },
    )
    .await?;
    Ok(ApiResponse::success(receipt))
}

async fn export_job_workspace(db: &sea_orm::DatabaseConnection, job_id: Uuid) -> Result<Uuid, ApiError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id FROM flow_export_jobs WHERE id=$1",
            vec![job_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("export job not found".to_string()))?;
    row.try_get("", "workspace_id").map_err(Into::into)
}

pub async fn get_flow_export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = export_job_workspace(&state.db, job_id).await?;
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, role, is_bot) = require_workspace_access(&state, &extensions, workspace_id).await?;
    let receipt =
        crate::flow::export::get_package_export(&state.db, job_id, &package_export_principal(actor_id, role, is_bot))
            .await?;
    let mut data = serde_json::to_value(receipt).map_err(|_| ApiError::Internal)?;
    data.as_object_mut().ok_or(ApiError::Internal)?.insert(
        "download_url".to_string(),
        json!(format!("/api/v1/flow/exports/{job_id}/artifact")),
    );
    Ok(ApiResponse::success(data))
}

pub async fn get_flow_export_artifact(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = export_job_workspace(&state.db, job_id).await?;
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, role, is_bot) = require_workspace_access(&state, &extensions, workspace_id).await?;
    let (bytes, hash, format) = crate::flow::export::download_package_export(
        &state.db,
        job_id,
        &package_export_principal(actor_id, role, is_bot),
    )
    .await?;
    let mut headers = HeaderMap::new();
    let content_type = match format.as_str() {
        "json" => "application/json; charset=utf-8",
        "markdown" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "package" => "application/vnd.sylvode.flow-package+zip;version=1",
        _ => return Err(ApiError::Internal),
    };
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        "x-flow-package-sha256",
        HeaderValue::from_str(&hash).map_err(|_| ApiError::Internal)?,
    );
    Ok((headers, bytes))
}

pub async fn post_flow_import_artifact(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    request: Request,
) -> Result<impl IntoResponse, ApiError> {
    require_package_tool(bot.as_ref(), "objects.import_artifact")?;
    let (principal, _) = package_workspace_principal(&state, claims, bot, workspace_id).await?;
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let multipart_idempotency_key = request
        .headers()
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let (artifact_id, package_sha256, expires_at) =
        if content_type.starts_with("multipart/form-data") {
            let mut multipart = Multipart::from_request(request, &state)
                .await
                .map_err(|_| ApiError::BadRequest("invalid multipart package upload".to_string()))?;
            let mut stager = crate::flow::import::BoundedImportStager::new(Vec::new(), std::io::sink());
            let mut seen = false;
            while let Some(mut field) = multipart
                .next_field()
                .await
                .map_err(|_| ApiError::invalid_update("multipart package stream failed"))?
            {
                if field.name() != Some("package") || seen {
                    return Err(ApiError::BadRequest(
                        "multipart upload requires exactly one package field".to_string(),
                    ));
                }
                seen = true;
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| ApiError::invalid_update("multipart package stream failed"))?
                {
                    stager.write_archive_chunk(&chunk)?;
                }
            }
            if !seen {
                return Err(ApiError::BadRequest("multipart package field is missing".to_string()));
            }
            stager.finish_archive()?;
            let (bytes, _) = stager.into_stages()?;
            crate::flow::package_import::upload_package_artifact(
                &state.db,
                workspace_id,
                &principal,
                bytes,
                None,
                &multipart_idempotency_key,
            )
            .await?
        } else {
            let Json(req) = Json::<PackageArtifactRequest>::from_request(request, &state)
                .await
                .map_err(|_| ApiError::BadRequest("invalid import artifact JSON".to_string()))?;
            if req.idempotency_key.trim().is_empty() {
                return Err(ApiError::BadRequest(
                    "JSON artifact source requires idempotency_key".to_string(),
                ));
            }
            match req.source.kind.as_str() {
                "inline_base64" => {
                    let encoded = req.source.package_base64.as_deref().ok_or_else(|| {
                        ApiError::BadRequest("inline_base64 source requires package_base64".to_string())
                    })?;
                    if req.source.object_key.is_some() || req.source.size.is_some() {
                        return Err(ApiError::BadRequest(
                            "inline_base64 source cannot carry staged object fields".to_string(),
                        ));
                    }
                    crate::flow::package_import::upload_inline_base64_artifact(
                        &state.db,
                        workspace_id,
                        &principal,
                        encoded.as_bytes(),
                        req.source.package_sha256.as_deref(),
                        &req.idempotency_key,
                    )
                    .await?
                }
                "staged_object" => {
                    if req.source.package_base64.is_some() {
                        return Err(ApiError::BadRequest(
                            "staged_object source cannot carry package_base64".to_string(),
                        ));
                    }
                    let object_key =
                        req.source.object_key.as_deref().ok_or_else(|| {
                            ApiError::BadRequest("staged_object source requires object_key".to_string())
                        })?;
                    let expected_size = req
                        .source
                        .size
                        .ok_or_else(|| ApiError::BadRequest("staged_object source requires size".to_string()))?;
                    let expected_hash = req.source.package_sha256.as_deref().ok_or_else(|| {
                        ApiError::BadRequest("staged_object source requires package_sha256".to_string())
                    })?;
                    let trusted_prefix = format!("flow-package-staging/{workspace_id}/");
                    if !object_key.starts_with(&trusted_prefix) {
                        return Err(ApiError::Forbidden(
                            "staged package object is not bound to the target workspace".to_string(),
                        ));
                    }
                    let bytes = crate::services::object_storage::ObjectStorage::from_runtime_config()?
                        .get(object_key)
                        .await?;
                    if u64::try_from(bytes.len()).ok() != Some(expected_size) {
                        return Err(ApiError::BadRequest("staged package size does not match".to_string()));
                    }
                    crate::flow::package_import::upload_package_artifact(
                        &state.db,
                        workspace_id,
                        &principal,
                        bytes,
                        Some(expected_hash),
                        &req.idempotency_key,
                    )
                    .await?
                }
                _ => {
                    return Err(ApiError::BadRequest(
                        "artifact source kind must be inline_base64 or staged_object".to_string(),
                    ));
                }
            }
        };
    let size: i64 = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT size_bytes FROM flow_package_artifacts WHERE id=$1",
            vec![artifact_id.into()],
        ))
        .await?
        .ok_or(ApiError::Internal)?
        .try_get("", "size_bytes")?;
    Ok(ApiResponse::success(
        json!({"artifact_id":artifact_id,"package_sha256":package_sha256,"size":size,"expires_at":expires_at}),
    ))
}

pub async fn post_flow_import_preview(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PackageImportPreviewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_package_tool(bot.as_ref(), "objects.import_preview")?;
    let (principal, _) = package_workspace_principal(&state, claims, bot, workspace_id).await?;
    let receipt = crate::flow::package_import::preview_package_import(
        &state.db,
        &crate::flow::package_import::PreviewImportRequest {
            workspace_id,
            artifact_id: req.artifact_id,
            project_map: req.project_mapping,
            external_reference_policy: req.external_reference_policy,
            conflict_policy: req.conflict_policy,
            include_history: req.include_history,
            idempotency_key: req.idempotency_key,
            principal,
        },
    )
    .await?;
    Ok(ApiResponse::success(receipt))
}

pub async fn post_flow_import_commit(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, import_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<PackageImportCommitRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_package_tool(bot.as_ref(), "objects.import_commit")?;
    let (principal, _) = package_workspace_principal(&state, claims, bot, workspace_id).await?;
    let report = crate::flow::package_import::commit_package_import(
        &state.db,
        &crate::flow::package_import::CommitImportRequest {
            workspace_id,
            preview_id: import_id,
            package_sha256: req.package_sha256,
            mapping_hash: req.mapping_hash,
            conflict_policy: req.conflict_policy,
            confirm: req.confirm,
            idempotency_key: req.idempotency_key,
            principal,
        },
    )
    .await?;
    Ok(ApiResponse::success(
        json!({"job_id":report.import_id,"import_id":report.import_id,"status":report.status}),
    ))
}

pub async fn get_flow_import(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, import_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let (principal, _) = package_workspace_principal(&state, claims, bot, workspace_id).await?;
    Ok(ApiResponse::success(
        crate::flow::package_import::get_import_report(&state.db, workspace_id, import_id, &principal).await?,
    ))
}

/// `POST /api/v1/flow/objects/{object_id}/references`.
pub async fn post_flow_object_reference(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(input): Json<crate::flow::bridge::CreateReferenceInput>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let receipt =
        crate::flow::bridge::create_reference(&state, &extensions, object_id, input, request_origin(&extensions))
            .await?;
    Ok(ApiResponse::success(receipt))
}

/// Request-time permission resolution for Reference cards and Embed views.
pub async fn get_flow_object_references(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(
        crate::flow::bridge::list_references(&state, &extensions, object_id).await?,
    ))
}

/// `DELETE /api/v1/flow/objects/{object_id}/references/{reference_id}`.
pub async fn delete_flow_object_reference(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object_id, reference_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let extensions = build_auth_extensions(claims, bot);
    let receipt = crate::flow::bridge::remove_reference(
        &state,
        &extensions,
        object_id,
        reference_id,
        key,
        request_origin(&extensions),
    )
    .await?;
    Ok(ApiResponse::success(receipt))
}

pub async fn post_flow_conversion_preview(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(input): Json<crate::flow::bridge::ConversionPreviewInput>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(
        crate::flow::bridge::preview_conversion(&state, &extensions, input).await?,
    ))
}

pub async fn post_flow_conversion(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(input): Json<crate::flow::bridge::ConversionCommitInput>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(
        crate::flow::bridge::commit_conversion(&state, &extensions, input, request_origin(&extensions)).await?,
    ))
}

pub async fn get_flow_conversion(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(
        crate::flow::bridge::conversion_status(&state, &extensions, job_id).await?,
    ))
}

pub async fn get_flow_conversion_owner(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(conversion_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(
        crate::flow::bridge::conversion_owner(&state, &extensions, conversion_id).await?,
    ))
}

pub async fn post_flow_conversion_retry(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job_id): Path<Uuid>,
    Json(input): Json<crate::flow::bridge::ConversionRetryInput>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(
        crate::flow::bridge::retry_conversion(&state, &extensions, job_id, input, request_origin(&extensions)).await?,
    ))
}

/// The origin every write handler in this module stamps on the events its command produces.
///
/// **This is the point of the whole `CommandOrigin` plumbing**, and the one place any Flow route
/// decides what surface it is.
///
/// `events-v1.md` freezes "`source` 由服务端按 Web/REST/MCP/CLI/worker 覆盖", and as of 2026-09-01
/// adds the clause that makes this function's shape non-negotiable: "**`source` 必须由认证/传输
/// 边界推导，不得由调用方自报**……surface、server session、exact registered tool 全部取自中间件
/// 已解析出的可信上下文".
///
/// # Why a resolver and not a constant
///
/// The first version of this fix moved the hardcoded `json!({ "surface": "rest" })` out of the
/// producers in `flow::command` / `flow::move_object` / `flow::grants` and into a single
/// REST-returning helper here. That was only half a fix, and the contract now says so in as many
/// words: "把 surface 变成一个参数**并不等于**修好了它——如果 handler 仍然无条件填一个常量，只是把
/// 写死从 producer 挪到了 route 层". `flow.feature_set` is a **registered, in-use MCP tool**
/// (`apps/mcp-server/src/tools/mod.rs`) whose client already sends `X-OpenPR-MCP-Surface` and
/// `X-OpenPR-MCP-Tool` (`apps/mcp-server/src/client/mod.rs`), and `middleware::bot_auth` already
/// parsed both — then spent them on the bot-operation log and dropped them. So every real MCP
/// call through these routes was still recorded as `rest`. The defect this whole work package
/// exists to fix was, for the one caller that actually exercises it today, not fixed at all.
///
/// # The resolution
///
/// | credential | surface | `request` | `tool` |
/// |---|---|---|---|
/// | bot token (MCP/CLI) | [`BotAuthContext::surface`], allow-listed at the boundary | the middleware's own `request_id`, shared with the `bot_operation_logs` row | the exact registered tool name, when the call is a tool call |
/// | JWT direct | [`EventSurface::Rest`] | a per-request UUID minted here | omitted — REST has no tool concept |
///
/// `session`/`client_id`/`service` are omitted for both: MCP-over-HTTP plumbs no server session id
/// to this process, `client_id` is the WebSocket ticket handshake's field (see
/// `flow::collab::session`, the only other surface declaration point in the system), and `service`
/// is reserved for `surface=system` background work. `events-v1.md`: "不适用时省略且不能填 caller
/// 自报值" — so they are absent keys, not empty strings.
///
/// `correlation_id` is minted per request rather than reusing `request`: they answer different
/// questions ("which HTTP call" vs "which causal chain"), and `events-v1.md` keeps them as
/// separate envelope fields. Every event this one request writes — the command's primary
/// transition and every event derived from it — carries this same value.
///
/// **Every** Flow write path resolves its origin through this one function; there is deliberately
/// no second, `authorization_caller`-shaped bypass that fills a constant of its own.
fn request_origin(extensions: &axum::http::Extensions) -> CommandOrigin {
    let source = crate::middleware::bot_auth::extract_bot_context(extensions).map_or_else(
        || EventSource::new(EventSurface::Rest).with_request(Uuid::new_v4().to_string()),
        |bot| {
            // ADR-0019 AO-1: middleware constructed this context only after the presented
            // transport matched the surface registered with the bot credential.
            let source = EventSource::new(bot.surface).with_request(bot.request_id.to_string());
            match bot.tool_name.as_deref() {
                Some(tool) => source.with_tool(tool),
                None => source,
            }
        },
    );
    CommandOrigin::first_request(source)
}

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReplayDeliveriesRequest {
    pub mode: crate::events::dispatcher::ReplayMode,
    pub event_type: Option<String>,
    pub subscriber_kind: Option<String>,
    pub subscriber_id: Option<Uuid>,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub dry_run: bool,
    pub confirm: bool,
    pub idempotency_key: String,
}

#[derive(Debug, FromQueryResult)]
struct ReplayIdempotencyRow {
    request_hash: String,
    status: String,
    result_redacted: Option<Value>,
}

/// `POST /api/v1/admin/workspaces/{workspace_id}/flow/deliveries/replay`.
pub async fn post_flow_delivery_replay(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<ReplayDeliveriesRequest>,
) -> Result<axum::response::Response, ApiError> {
    if !req.confirm || req.idempotency_key.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "confirm=true and a non-empty idempotency_key are required".to_string(),
        ));
    }
    if let Some(Extension(context)) = &bot
        && context.tool_name.as_deref() != Some("deliveries.replay")
    {
        return Err(ApiError::Forbidden(
            "delivery replay requires the exact deliveries.replay tool policy".to_string(),
        ));
    }
    let extensions = build_auth_extensions(claims, bot);
    let (principal_id, _role, is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;
    let principal_kind = if is_bot { "bot" } else { "user" };
    let request_bytes = serde_json::to_vec(&req).map_err(|_| ApiError::Internal)?;
    let request_hash = format!("{:x}", Sha256::digest(request_bytes));

    let inserted = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_replay_requests \
               (workspace_id,principal_kind,principal_id,idempotency_key,request_hash) \
             VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
            vec![
                workspace_id.into(),
                principal_kind.into(),
                principal_id.into(),
                req.idempotency_key.clone().into(),
                request_hash.clone().into(),
            ],
        ))
        .await?
        .rows_affected();
    if inserted == 0 {
        let prior = ReplayIdempotencyRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT request_hash,status,result_redacted FROM flow_replay_requests \
              WHERE workspace_id=$1 AND principal_kind=$2 AND principal_id=$3 AND idempotency_key=$4",
            vec![
                workspace_id.into(),
                principal_kind.into(),
                principal_id.into(),
                req.idempotency_key.into(),
            ],
        ))
        .one(&state.db)
        .await?
        .ok_or(ApiError::Internal)?;
        if prior.request_hash != request_hash {
            return Err(ApiError::Conflict("idempotency key body drift".to_string()));
        }
        if prior.status != "completed" {
            return Err(ApiError::Conflict(
                "identical replay request is still running".to_string(),
            ));
        }
        return Ok(ApiResponse::success(prior.result_redacted.ok_or(ApiError::Internal)?).into_response());
    }

    let request = crate::events::dispatcher::ReplayRequest {
        workspace_id,
        mode: req.mode,
        event_type: req.event_type,
        subscriber_kind: req.subscriber_kind,
        subscriber_id: req.subscriber_id,
        from: req.from,
        to: req.to,
        dry_run: req.dry_run,
    };
    let result = match crate::events::dispatcher::replay_deliveries(&state.db, &request, Utc::now()).await {
        Ok(result) => result,
        Err(error) => {
            let _ = state
                .db
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "DELETE FROM flow_replay_requests \
                      WHERE workspace_id=$1 AND principal_kind=$2 AND principal_id=$3 \
                        AND idempotency_key=$4 AND status='running'",
                    vec![
                        workspace_id.into(),
                        principal_kind.into(),
                        principal_id.into(),
                        req.idempotency_key.into(),
                    ],
                ))
                .await;
            return Err(error);
        }
    };
    let result_json = serde_json::to_value(&result).map_err(|_| ApiError::Internal)?;
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_replay_requests SET status='completed',result_redacted=$5,finished_at=now() \
              WHERE workspace_id=$1 AND principal_kind=$2 AND principal_id=$3 AND idempotency_key=$4",
            vec![
                workspace_id.into(),
                principal_kind.into(),
                principal_id.into(),
                req.idempotency_key.into(),
                result_json.into(),
            ],
        ))
        .await?;
    Ok(ApiResponse::success(result).into_response())
}

fn require_exact_admin_tool(bot: Option<&Extension<BotAuthContext>>, expected: &str) -> Result<(), ApiError> {
    if let Some(Extension(context)) = bot
        && context.tool_name.as_deref() != Some(expected)
    {
        return Err(ApiError::Forbidden(format!(
            "operation requires the exact {expected} tool policy"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod v08_admin_tool_tests {
    use super::{BotAuthContext, Extension, require_exact_admin_tool};
    use uuid::Uuid;

    fn bot(tool_name: Option<&str>) -> Extension<BotAuthContext> {
        Extension(BotAuthContext {
            bot_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            permissions: vec!["admin".to_string()],
            surface: crate::flow::event_origin::EventSurface::McpHttp,
            tool_name: tool_name.map(str::to_string),
            request_id: Uuid::new_v4(),
        })
    }

    #[test]
    fn dangerous_admin_tools_require_an_exact_registered_name_without_blocking_native_users() {
        let correct = bot(Some("collab.compact"));
        let wrong = bot(Some("collab.rebuild_projection"));
        let missing = bot(None);
        assert!(require_exact_admin_tool(Some(&correct), "collab.compact").is_ok());
        assert!(require_exact_admin_tool(Some(&wrong), "collab.compact").is_err());
        assert!(require_exact_admin_tool(Some(&missing), "collab.compact").is_err());
        assert!(require_exact_admin_tool(None, "collab.compact").is_ok());
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompactDocumentRequest {
    pub dry_run: bool,
    pub expected_head_seq: Option<i64>,
    pub retain_after_seq: Option<i64>,
    pub confirm_document_id: Option<Uuid>,
    pub idempotency_key: String,
}

pub async fn post_flow_compact_document(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(document_id): Path<Uuid>,
    Json(req): Json<CompactDocumentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_exact_admin_tool(bot.as_ref(), "collab.compact")?;
    let extensions = build_auth_extensions(claims, bot);
    let scope = crate::flow::operations::document_scope(&state.db, document_id).await?;
    let (principal_id, _role, is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, scope.workspace_id).await?;
    let expected = req
        .expected_head_seq
        .ok_or_else(|| ApiError::BadRequest("expected_head_seq is required".to_string()))?;
    if !req.dry_run && req.confirm_document_id != Some(document_id) {
        return Err(ApiError::Forbidden(
            "execute requires confirm_document_id matching the path document".to_string(),
        ));
    }
    if req.retain_after_seq.is_some_and(|seq| seq != expected) {
        return Err(ApiError::BadRequest(
            "retain_after_seq currently must equal expected_head_seq".to_string(),
        ));
    }
    Ok(ApiResponse::success(
        crate::flow::operations::compact_document(
            &state.db,
            &scope,
            req.dry_run,
            expected,
            !req.dry_run && req.retain_after_seq.is_none(),
            crate::flow::operations::Principal {
                id: principal_id,
                is_bot,
            },
            &req.idempotency_key,
        )
        .await?,
    ))
}

#[derive(Debug, Clone, Deserialize)]
pub struct RebuildProjectionRequest {
    pub dry_run: bool,
    pub expected_head_seq: Option<i64>,
    pub confirm_object_id: Option<Uuid>,
    pub idempotency_key: String,
}

pub async fn post_flow_rebuild_projection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(req): Json<RebuildProjectionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_exact_admin_tool(bot.as_ref(), "collab.rebuild_projection")?;
    let extensions = build_auth_extensions(claims, bot);
    let scope = crate::flow::operations::object_scope(&state.db, object_id).await?;
    let (principal_id, _role, is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, scope.workspace_id).await?;
    let expected = req
        .expected_head_seq
        .ok_or_else(|| ApiError::BadRequest("expected_head_seq is required".to_string()))?;
    if !req.dry_run && req.confirm_object_id != Some(object_id) {
        return Err(ApiError::Forbidden(
            "execute requires confirm_object_id matching the path object".to_string(),
        ));
    }
    Ok(ApiResponse::success(
        crate::flow::operations::rebuild_projection(
            &state.db,
            &scope,
            req.dry_run,
            expected,
            crate::flow::operations::Principal {
                id: principal_id,
                is_bot,
            },
            &req.idempotency_key,
        )
        .await?,
    ))
}

#[derive(Debug, Clone, Deserialize)]
pub struct VerifyDocumentRequest {
    pub dry_run: bool,
    pub deep: bool,
    pub expected_head_seq: Option<i64>,
    pub idempotency_key: String,
}

pub async fn post_flow_verify_document(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(document_id): Path<Uuid>,
    Json(req): Json<VerifyDocumentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_exact_admin_tool(bot.as_ref(), "objects.integrity")?;
    if !req.dry_run {
        return Err(ApiError::BadRequest("document verify is dry_run only".to_string()));
    }
    let extensions = build_auth_extensions(claims, bot);
    let scope = crate::flow::operations::document_scope(&state.db, document_id).await?;
    let (principal_id, _role, is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, scope.workspace_id).await?;
    let fingerprint = crate::flow::collab::integrity::document_fingerprint(&state.db, document_id).await?;
    let expected = req.expected_head_seq.unwrap_or(fingerprint.head_seq);
    Ok(ApiResponse::success(
        crate::flow::operations::verify_document(
            &state.db,
            &scope,
            expected,
            req.deep,
            crate::flow::operations::Principal {
                id: principal_id,
                is_bot,
            },
            &req.idempotency_key,
        )
        .await?,
    ))
}

pub async fn get_flow_admin_health(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    require_exact_admin_tool(bot.as_ref(), "collab.status")?;
    let extensions = build_auth_extensions(claims, bot);
    policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;
    Ok(ApiResponse::success(
        crate::flow::operations::workspace_health(&state.db, workspace_id).await?,
    ))
}

pub async fn get_flow_admin_lag(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    require_exact_admin_tool(bot.as_ref(), "collab.status")?;
    let extensions = build_auth_extensions(claims, bot);
    policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;
    Ok(ApiResponse::success(
        crate::flow::operations::workspace_lag(&state.db, workspace_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct AdminIntegrityQuery {
    pub scope: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

pub async fn get_flow_admin_integrity(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<AdminIntegrityQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_exact_admin_tool(bot.as_ref(), "objects.integrity")?;
    let extensions = build_auth_extensions(claims, bot);
    policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;
    if query.cursor.is_some() {
        return Err(ApiError::BadRequest(
            "integrity cursor is not available on the first v0.8 page".to_string(),
        ));
    }
    let include_documents = match query.scope.as_deref().unwrap_or("summary") {
        "summary" => false,
        "documents" => true,
        _ => return Err(ApiError::BadRequest("scope must be summary or documents".to_string())),
    };
    Ok(ApiResponse::success(
        crate::flow::operations::workspace_integrity(
            &state.db,
            workspace_id,
            include_documents,
            query.limit.unwrap_or(50),
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateFlowObjectRequest {
    pub object_type: String,
    pub project_id: Option<Uuid>,
    pub parent_object_id: Option<Uuid>,
    pub title: String,
    pub idempotency_key: String,
    pub message: Option<String>,
    #[serde(default)]
    pub initial_fields: Vec<Value>,
    pub initial_view: Option<Value>,
}

/// `POST /api/v1/workspaces/{workspace_id}/flow/objects`
pub async fn create_flow_object(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateFlowObjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, _role, actor_is_bot) =
        policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    let input = CreateObjectInput {
        workspace_id,
        actor_id,
        actor_is_bot,
        object_type: req.object_type,
        project_id: req.project_id,
        parent_object_id: req.parent_object_id,
        title: req.title,
        idempotency_key: req.idempotency_key,
        message: req.message,
        origin: request_origin(&extensions),
    };
    let accepted = if req.initial_fields.is_empty() && req.initial_view.is_none() {
        crate::flow::command::create_object(&state, input).await?
    } else {
        crate::flow::command::create_object_with_collection_schema(
            &state,
            input,
            serde_json::json!({"initial_fields": req.initial_fields, "initial_view": req.initial_view}),
        )
        .await?
    };

    Ok(ApiResponse::success(accepted))
}

#[derive(Debug, Deserialize)]
pub struct ListFlowObjectsQuery {
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub unprojected: bool,
    pub object_type: Option<String>,
    pub parent_id: Option<Uuid>,
    pub q: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
    #[serde(default)]
    pub include_archived: bool,
}

/// `GET /api/v1/workspaces/{workspace_id}/flow/objects`
pub async fn list_flow_objects(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<ListFlowObjectsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let access = policy::begin_flow_read(&state, &extensions, workspace_id).await?;
        let Some(response) = query::list_objects(
            &state,
            &access,
            query::ListObjectsParams {
                workspace_id,
                project_id: params.project_id,
                unprojected: params.unprojected,
                object_type: params.object_type.clone(),
                parent_id: params.parent_id,
                q: params.q.clone(),
                cursor: params.cursor.clone(),
                limit: params.limit,
                include_archived: params.include_archived,
            },
        )
        .await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(response));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct GetFlowObjectQuery {
    pub at_seq: Option<i64>,
    pub render: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DescribeCollectionQuery {
    pub at_seq: Option<i64>,
}

async fn collection_read_access(
    state: &AppState,
    extensions: &axum::http::Extensions,
    collection_id: Uuid,
) -> Result<Option<policy::AuthorizedFlowObject>, ApiError> {
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, collection_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("collection not found".to_string()))?;
    policy::require_flow_object_access(
        state,
        extensions,
        workspace_id,
        collection_id,
        crate::flow::collab::authz::PermissionLevel::View,
    )
    .await
}

/// `GET /api/v1/flow/collections/{collection_id}` and the Collection branch of the schema
/// resource. Both are served from synchronous projections; neither decodes collab bytes.
pub async fn get_flow_collection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection_id): Path<Uuid>,
    Query(params): Query<DescribeCollectionQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = collection_read_access(&state, &extensions, collection_id).await? else {
            continue;
        };
        let Some(description) = collections::describe_collection(&state, &access, params.at_seq).await? else {
            continue;
        };
        return Ok(ApiResponse::success(description));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct GetCollectionRecordsQuery {
    pub filter: Option<String>,
    pub sort: Option<String>,
    pub group: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub field_ids: Option<String>,
}

fn parse_json_query<T: for<'de> Deserialize<'de>>(raw: Option<String>, name: &str) -> Result<Option<T>, ApiError> {
    raw.map(|raw| serde_json::from_str(&raw).map_err(|_| ApiError::invalid_update(format!("{name} is not valid JSON"))))
        .transpose()
}

fn parse_field_ids(raw: Option<String>) -> Result<Vec<Uuid>, ApiError> {
    raw.map_or_else(
        || Ok(Vec::new()),
        |raw| {
            raw.split(',')
                .filter(|part| !part.is_empty())
                .map(|part| {
                    Uuid::parse_str(part)
                        .map_err(|_| ApiError::invalid_update("field_ids must be comma-separated UUIDs"))
                })
                .collect()
        },
    )
}

/// `GET /api/v1/flow/collections/{collection_id}/records`.
pub async fn get_flow_collection_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection_id): Path<Uuid>,
    Query(params): Query<GetCollectionRecordsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let request = RecordQueryPayload {
        filter: parse_json_query(params.filter, "filter")?,
        sort: parse_json_query(params.sort, "sort")?,
        group: params.group,
        cursor: params.cursor,
        limit: params.limit.unwrap_or(50),
        field_ids: parse_field_ids(params.field_ids)?,
    };
    query_flow_collection(&state, claims, bot, collection_id, &request).await
}

async fn query_flow_collection(
    state: &AppState,
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    collection_id: Uuid,
    request: &RecordQueryPayload,
) -> Result<axum::response::Response, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = collection_read_access(state, &extensions, collection_id).await? else {
            continue;
        };
        let Some(response) = collections::query_collection_records(state, &access, request).await? else {
            continue;
        };
        return Ok(ApiResponse::success(response).into_response());
    }
    Err(policy::authorization_read_unstable())
}

/// `POST /api/v1/flow/collections/{collection_id}/query`.
pub async fn post_flow_collection_query(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection_id): Path<Uuid>,
    Json(request): Json<RecordQueryPayload>,
) -> Result<impl IntoResponse, ApiError> {
    query_flow_collection(&state, claims, bot, collection_id, &request).await
}

#[derive(Debug, Deserialize)]
pub struct CreateCollectionRecordRequest {
    pub values_by_field_id: serde_json::Map<String, Value>,
    pub body: Option<String>,
    pub idempotency_key: String,
    pub message: Option<String>,
}

/// `POST /api/v1/flow/collections/{collection_id}/records`.
pub async fn post_flow_collection_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection_id): Path<Uuid>,
    Json(req): Json<CreateCollectionRecordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, collection_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("collection not found".to_string()))?;
    let (actor_id, role, is_bot) = policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;
    let accepted = crate::flow::command::execute_command(
        &state,
        ExecuteCommandInput {
            object_id: collection_id,
            actor_id,
            principal_kind: if is_bot { "bot".to_string() } else { "user".to_string() },
            role,
            command_type: "record_create".to_string(),
            payload: serde_json::json!({"properties": req.values_by_field_id, "body": req.body}),
            expected_frontier: None,
            idempotency_key: req.idempotency_key,
            message: req.message,
            origin_client_id: format!("rest:{actor_id}"),
            origin: request_origin(&extensions),
        },
    )
    .await?;
    Ok(ApiResponse::success(accepted))
}

#[derive(Debug, Deserialize)]
pub struct GetFlowNavigatorQuery {
    pub project_id: Option<Uuid>,
    pub depth: Option<u64>,
    #[serde(default)]
    pub include_archived: bool,
}

/// `GET /api/v1/workspaces/{workspace_id}/flow/navigator`.
pub async fn get_flow_navigator(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<GetFlowNavigatorQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let access = policy::begin_flow_read(&state, &extensions, workspace_id).await?;
        let Some(response) = query::get_navigator(
            &state,
            &access,
            params.project_id,
            params.depth,
            params.include_archived,
        )
        .await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(response));
    }
    Err(policy::authorization_read_unstable())
}

/// `GET /api/v1/flow/objects/{object_id}`
pub async fn get_flow_object(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Query(params): Query<GetFlowObjectQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let render = Render::parse(params.render.as_deref())?;
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = policy::require_flow_object_access(
            &state,
            &extensions,
            workspace_id,
            object_id,
            crate::flow::collab::authz::PermissionLevel::View,
        )
        .await?
        else {
            continue;
        };
        let Some(view) = query::get_object(&state, &access, params.at_seq, render).await? else {
            continue;
        };
        return Ok(ApiResponse::success(view));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct GetFlowObjectBootstrapQuery {
    pub known_seq: Option<i64>,
    pub known_frontier: Option<String>,
}

/// `GET /api/v1/flow/objects/{object_id}/bootstrap` (`rest-api-v1.md`: "**user only**；object
/// read/write；flag").
///
/// Unlike every other handler in this module, a bot token is rejected outright rather than
/// folded into the workspace-access check — matching `routes::collab::create_ticket`'s identical
/// "issued a `bot_or_user_auth_middleware`-gated route but this one endpoint is user-only" shape.
/// `flow::query::get_bootstrap` shares `flow::collab::bootstrap::load` with the WebSocket
/// `snapshot` frame, so this and a WS `open` on the same document can never diverge (`ADR-0010`).
pub async fn get_flow_object_bootstrap(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Query(params): Query<GetFlowObjectBootstrapQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if bot.is_some() {
        return Err(ApiError::Forbidden(
            "bot tokens cannot call the bootstrap endpoint; user access token only".to_string(),
        ));
    }
    let extensions = build_auth_extensions(claims, None);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = policy::require_flow_object_access(
            &state,
            &extensions,
            workspace_id,
            object_id,
            crate::flow::collab::authz::PermissionLevel::Edit,
        )
        .await?
        else {
            continue;
        };
        let Some(bootstrap) =
            query::get_bootstrap(&state, &access, params.known_seq, params.known_frontier.clone()).await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(bootstrap));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct FlowCommandEnvelope {
    #[serde(rename = "type")]
    pub command_type: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteFlowCommandRequest {
    pub command: FlowCommandEnvelope,
    #[serde(default)]
    pub expected_frontier: Option<String>,
    pub idempotency_key: String,
    pub message: Option<String>,
}

/// `POST /api/v1/flow/objects/{object_id}/commands`, shared by content, lifecycle, governance,
/// and v0.6 Collection command families.
///
/// `command::execute_command` re-runs the object-level `edit`/`full_access` permission check
/// itself (`authz::effective_permission`) on top of the workspace-membership gate here — the same
/// split `flow::collab::ticket::issue` uses for the WebSocket path (workspace access, then a
/// separate object-level check), since `OwnedBy(FlowObject)` is stricter than plain membership.
pub async fn post_flow_object_command(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(req): Json<ExecuteFlowCommandRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let (actor_id, role, is_bot) = policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    // This surface has no client-id handshake like the WebSocket ticket flow (`ADR-0007`), so a
    // stable per-actor tag is synthesized for `collab_updates.origin_client_id` / the relayed
    // `update` frame's `origin` field — descriptive metadata only, never an authority.
    let origin_client_id = format!("rest:{actor_id}");

    let accepted = crate::flow::command::execute_command(
        &state,
        ExecuteCommandInput {
            object_id,
            actor_id,
            principal_kind: if is_bot { "bot".to_string() } else { "user".to_string() },
            role,
            command_type: req.command.command_type,
            payload: req.command.payload,
            expected_frontier: req.expected_frontier,
            idempotency_key: req.idempotency_key,
            message: req.message,
            origin_client_id,
            origin: request_origin(&extensions),
        },
    )
    .await?;

    Ok(ApiResponse::success(accepted))
}

#[derive(Debug, Deserialize)]
pub struct FlowRelationsQuery {
    pub direction: Option<String>,
    pub relation_type: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// `GET /api/v1/flow/objects/{object_id}/relations`.
///
/// The root object is authorized here through the standard `OwnedBy(FlowObject)` read context;
/// `flow::relations` reauthorizes every opposite endpoint at the same epoch and performs the
/// final epoch check before exposing the page.
pub async fn get_flow_object_relations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Query(params): Query<FlowRelationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let direction = relations::RelationDirection::parse(params.direction.as_deref())?;
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = policy::require_flow_object_access(
            &state,
            &extensions,
            workspace_id,
            object_id,
            crate::flow::collab::authz::PermissionLevel::View,
        )
        .await?
        else {
            continue;
        };
        let Some(page) = relations::list_relations(
            &state,
            &access,
            relations::ListRelationsParams {
                direction,
                relation_type: params.relation_type.clone(),
                cursor: params.cursor.clone(),
                limit: params.limit,
            },
        )
        .await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(page));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct FlowObjectHistoryQuery {
    pub before_seq: Option<i64>,
    pub limit: Option<u64>,
}

/// `GET /api/v1/flow/objects/{object_id}/history`
pub async fn get_flow_object_history(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Query(params): Query<FlowObjectHistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = policy::require_flow_object_access(
            &state,
            &extensions,
            workspace_id,
            object_id,
            crate::flow::collab::authz::PermissionLevel::View,
        )
        .await?
        else {
            continue;
        };
        let Some(response) = query::get_history(&state, &access, params.before_seq, params.limit).await? else {
            continue;
        };
        return Ok(ApiResponse::success(response));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct FlowObjectDiffQuery {
    pub from_seq: Option<i64>,
    pub to_seq: Option<i64>,
    pub render: Option<String>,
}

/// `GET /api/v1/flow/objects/{object_id}/diff`.
///
/// This is a history read: object visibility is established through WP-10's shared
/// `require_flow_object_access` context and the query service rechecks its epoch after the
/// historical state has been reconstructed. An absent object and an object the caller cannot
/// view both produce the same `not_found` answer.
pub async fn get_flow_object_diff(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Query(params): Query<FlowObjectDiffQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let Some(access) = policy::require_flow_object_access(
            &state,
            &extensions,
            workspace_id,
            object_id,
            crate::flow::collab::authz::PermissionLevel::View,
        )
        .await?
        else {
            continue;
        };
        // Parse caller-controlled query values only after object visibility is established.
        // Otherwise malformed queries distinguish a hidden existing object (400) from an absent
        // object (404), turning this endpoint into an existence oracle.
        let from_seq = params
            .from_seq
            .ok_or_else(|| ApiError::invalid_update("from_seq is required"))?;
        let to_seq = params
            .to_seq
            .ok_or_else(|| ApiError::invalid_update("to_seq is required"))?;
        let render = Render::parse(params.render.as_deref())?;
        let Some(response) = query::get_object_diff(&state, &access, from_seq, to_seq, render).await? else {
            continue;
        };
        return Ok(ApiResponse::success(response));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct ProjectionLagQuery {
    pub project_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// `GET /api/v1/workspaces/{workspace_id}/flow/projection-lag`.
///
/// Workspace membership starts WP-10's shared read context; the domain layer batch-authorizes
/// every candidate object and rechecks the epoch before returning any page or aggregate.
pub async fn get_flow_projection_lag(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<ProjectionLagQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let access = policy::begin_flow_read(&state, &extensions, workspace_id).await?;
        let Some(response) = query::get_projection_lag(
            &state,
            &access,
            query::ProjectionLagParams {
                workspace_id,
                project_id: params.project_id,
                cursor: params.cursor.clone(),
                limit: params.limit,
            },
        )
        .await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(response));
    }
    Err(policy::authorization_read_unstable())
}

#[derive(Debug, Deserialize)]
pub struct FlowSearchQuery {
    pub q: String,
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub unprojected: bool,
    #[serde(default)]
    pub all_visible: bool,
    pub object_type: Option<String>,
    pub freshness: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// `GET /api/v1/workspaces/{workspace_id}/flow/search`.
///
/// The domain service validates the exclusive scope, evaluates a policy-filtered frontier, then
/// reauthorizes every matching candidate. An epoch change restarts the entire request so no page
/// can combine authorization decisions from two policy states.
pub async fn get_flow_search(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<FlowSearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    for _attempt in 0..policy::AUTHORIZATION_READ_ATTEMPTS {
        let access = policy::begin_flow_read(&state, &extensions, workspace_id).await?;
        let Some(response) = search::search(
            &state,
            &access,
            search::SearchParams {
                workspace_id,
                q: params.q.clone(),
                project_id: params.project_id,
                unprojected: params.unprojected,
                all_visible: params.all_visible,
                object_type: params.object_type.clone(),
                freshness: params.freshness.clone(),
                cursor: params.cursor.clone(),
                limit: params.limit,
            },
        )
        .await?
        else {
            continue;
        };
        return Ok(ApiResponse::success(response));
    }
    Err(policy::authorization_read_unstable())
}

/// `GET /api/v1/workspaces/{workspace_id}/features/flow`.
///
/// Plain workspace membership (`policy::require_flow_feature_read_access`), *not*
/// `require_flow_workspace_access` — this is the endpoint a caller uses to find out whether Flow
/// is enabled, so it must be readable even when the flag is currently `false`.
pub async fn get_flow_feature(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    policy::require_flow_feature_read_access(&state, &extensions, workspace_id).await?;

    let view = query::get_flow_feature(&state, workspace_id).await?;

    Ok(ApiResponse::success(view))
}

#[derive(Debug, Deserialize)]
pub struct SetFlowFeatureRequest {
    pub enabled: Option<bool>,
    pub default_member_level: Option<String>,
    pub idempotency_key: String,
}

/// `PUT /api/v1/workspaces/{workspace_id}/features/flow`.
///
/// Workspace admin only (`policy::require_flow_workspace_admin_access`) — `rest-api-v1.md`:
/// "workspace admin user 或 policy-approved Flow admin bot".
pub async fn set_flow_feature(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<SetFlowFeatureRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, _role, actor_is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;

    let view = crate::flow::command::set_flow_feature(
        &state,
        SetFlowFeatureInput {
            workspace_id,
            actor_id,
            actor_is_bot,
            enabled: req.enabled,
            default_member_level: req.default_member_level,
            idempotency_key: req.idempotency_key,
            origin: request_origin(&extensions),
        },
    )
    .await?;

    Ok(ApiResponse::success(view))
}

// ---- Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// These call the four handlers exactly the way the router does — through their public
// `axum::extract` signatures — against a real, freshly migrated PostgreSQL database, so a
// regression that only shows up once real SQL/real transactions run (a bad column name, a
// constraint violation, an `?` that should have been a typed error) fails here even though
// `cargo check` cannot see it. Matches the scratch-database convention already used by
// `apps/api/src/routes/form.rs`'s `record_link_database_tests` / `apps/api/src/main.rs`'s
// `proposal-scope-test` fixtures (maintenance connection string in, own throwaway database per
// run, migrated from `migrations/*.sql` on disk, dropped on the way out) — not the
// `apps/api/src/routes/label.rs` variant that treats the env var as an application connection
// directly, which is a separately tracked inconsistency this package does not touch.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing
)]
mod flow_database_tests {
    use std::io::{Cursor, Read, Write};
    use std::time::Duration;

    use super::{
        CompactDocumentRequest, GrantRequestBody, RebuildProjectionRequest, ReplayDeliveriesRequest, SetGrantsRequest,
        VerifyDocumentRequest, get_flow_admin_health, get_flow_admin_integrity, get_flow_admin_lag,
        post_flow_compact_document, post_flow_delivery_replay, post_flow_rebuild_projection, post_flow_verify_document,
    };
    use axum::body::to_bytes;
    use axum::response::{IntoResponse, Response};
    use base64::Engine as _;
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    use super::{
        CreateFlowObjectRequest, ExecuteFlowCommandRequest, FlowCommandEnvelope, FlowObjectDiffQuery,
        FlowObjectHistoryQuery, FlowRelationsQuery, FlowSearchQuery, GetFlowNavigatorQuery,
        GetFlowObjectBootstrapQuery, GetFlowObjectQuery, ListFlowObjectsQuery, ProjectionLagQuery,
        SetFlowFeatureRequest, SetInheritanceRequest, create_flow_object, delete_flow_object_reference,
        get_flow_conversion, get_flow_feature, get_flow_navigator, get_flow_object, get_flow_object_bootstrap,
        get_flow_object_diff, get_flow_object_grants, get_flow_object_history, get_flow_object_references,
        get_flow_object_relations, get_flow_projection_lag, get_flow_search, list_flow_objects, post_flow_conversion,
        post_flow_conversion_preview, post_flow_conversion_retry, post_flow_object_command, post_flow_object_reference,
        put_flow_object_grants, put_flow_object_inheritance, request_origin, set_flow_feature,
    };
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::{
        authz::PermissionLevel,
        permission_cache::{PermissionCache, PrincipalKind},
        registry::OutboundEvent,
    };
    use crate::routes::bot::{CreateBotRequest, create_bot};
    use crate::routes::form::{
        CreateFormRequest, CreateRecordRequest, UpdateFormPermissionsRequest, UpsertFormPermissionPolicy,
        create_form_record, create_project_form, delete_form, update_form_permissions,
    };
    use crate::routes::member::{
        AddMemberRequest, UpdateMemberRoleRequest, add_member, remove_member, update_member_role,
    };
    use axum::extract::{Path, Query, State};
    use axum::http::HeaderMap;
    use axum::{Extension, Json};

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    /// AO-1's transport-wide criterion at the production resolver. Middleware has already bound
    /// every represented surface to the bot credential before constructing this context.
    #[test]
    fn every_credential_bound_bot_transport_is_attested() {
        use crate::flow::event_origin::EventSurface;

        let direct = request_origin(&axum::http::Extensions::new()).source_json();
        assert_eq!(direct["attestation"], "attested");

        for surface in [
            EventSurface::Rest,
            EventSurface::McpHttp,
            EventSurface::McpSse,
            EventSurface::McpStdio,
            EventSurface::Cli,
            EventSurface::CliToolsCall,
        ] {
            let mut extensions = axum::http::Extensions::new();
            extensions.insert(crate::middleware::bot_auth::BotAuthContext {
                bot_id: Uuid::new_v4(),
                workspace_id: Uuid::new_v4(),
                permissions: vec!["write".to_string()],
                surface,
                tool_name: Some("flow.object_create".to_string()),
                request_id: Uuid::new_v4(),
            });
            let source = request_origin(&extensions).source_json();
            assert_eq!(source["surface"], surface.as_wire());
            assert_eq!(
                source["attestation"],
                "attested",
                "credential-bound bot surface {} must be attested",
                surface.as_wire()
            );
        }
    }

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_flow_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        migrate(&db).await;

        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "api-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-route-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    fn validator_ok_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (global $heap (mut i32) (i32.const 4096))
              (data (i32.const 1024) "{\"ok\":true}")
              (func (export "openpr_plugin_abi_version") (result i32)
                i32.const 1)
              (func (export "openpr_alloc") (param $len i32) (result i32)
                (local $ptr i32)
                global.get $heap
                local.set $ptr
                global.get $heap
                local.get $len
                i32.add
                global.set $heap
                local.get $ptr)
              (func (export "openpr_invoke") (param $ptr i32) (param $len i32) (result i64)
                i64.const 1024
                i64.const 32
                i64.shl
                i64.const 11
                i64.or))
            "#,
        )
        .expect("validator WAT should compile")
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    /// Seeds one workspace with an owner member and, unless `flow_enabled` is false, a
    /// `flow_workspace_settings` row turning the feature on for it.
    async fn seed_workspace(state: &AppState, flow_enabled: bool) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@flow.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'flow test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, $2)",
            vec![workspace_id.into(), flow_enabled.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    /// A workspace + owner member with **no** `flow_workspace_settings` row at all — unlike
    /// [`seed_workspace`], which always inserts one (`flow_enabled` true or false). Used by the
    /// `features/flow` "never provisioned" default test, where the row's mere absence (not an
    /// explicit `flow_enabled=false`) is exactly what is under test.
    async fn seed_bare_workspace(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@flow.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'flow test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    /// Adds a plain (`role='member'`, not `owner`/`admin`) member to an already-seeded workspace,
    /// for the `features/flow` admin-gate tests.
    async fn seed_member(state: &AppState, workspace_id: Uuid) -> Uuid {
        let member_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![member_id.into(), format!("{member_id}@flow.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![workspace_id.into(), member_id.into()],
        )
        .await;
        member_id
    }

    async fn seed_user(state: &AppState) -> Uuid {
        let user_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![user_id.into(), format!("{user_id}@flow.test").into()],
        )
        .await;
        user_id
    }

    async fn read_epoch(state: &AppState, workspace_id: Uuid) -> i64 {
        crate::flow::collab::authz::read_epoch(&state.db, workspace_id)
            .await
            .expect("flow authz epoch reads")
    }

    async fn create_page_as_owner(state: &AppState, workspace_id: Uuid, owner_id: Uuid, title: &str) -> Uuid {
        let body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: title.to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(body["code"], 0, "{body}");
        Uuid::parse_str(body["data"]["object"]["id"].as_str().expect("object id is a string"))
            .expect("object id is a uuid")
    }

    async fn document_of(state: &AppState, object_id: Uuid) -> Uuid {
        #[derive(FromQueryResult)]
        struct DocumentRow {
            id: Uuid,
        }
        DocumentRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id = $1",
            vec![object_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("document query runs")
        .expect("created page has a document")
        .id
    }

    fn claims_for(user_id: Uuid) -> Extension<JwtClaims> {
        Extension(JwtClaims {
            sub: user_id.to_string(),
            email: format!("{user_id}@flow.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        })
    }

    /// Normalizes a handler's `Result<impl IntoResponse, ApiError>` into a plain `Response`
    /// exactly as axum's own dispatch does, so tests observe precisely what a real HTTP client
    /// would receive.
    fn to_response<T: IntoResponse>(result: Result<T, ApiError>) -> Response {
        match result {
            Ok(ok) => ok.into_response(),
            Err(err) => err.into_response(),
        }
    }

    async fn body_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        serde_json::from_slice(&bytes).expect("response body is JSON")
    }

    /// The two v0.5 authorization routes mounted on **their real paths and methods**, with an
    /// `Extension<JwtClaims>` layer standing in for `bot_or_user_auth_middleware` (which is all
    /// that middleware contributes for a user caller).
    ///
    /// Everything past that point is the production stack: axum's own path routing, its `Json`
    /// extractor deserializing the raw request bytes, and the handler's own field mapping. Tests
    /// that construct `SetInheritanceRequest` in Rust and hand it to the handler as `Json(req)`
    /// skip the first two of those, and — the reason this exists — skip the handler's mapping
    /// line as well.
    fn authorization_router(state: AppState, caller_id: Uuid) -> axum::Router {
        axum::Router::new()
            .route(
                "/api/v1/flow/objects/{object_id}/grants",
                axum::routing::put(put_flow_object_grants),
            )
            .route(
                "/api/v1/flow/objects/{object_id}/inheritance",
                axum::routing::put(put_flow_object_inheritance),
            )
            .layer(claims_for(caller_id))
            .with_state(state)
    }

    /// Drives one real `PUT` through the router: real bytes in, real `Response` out.
    async fn http_put(app: &axum::Router, uri: &str, body: &str) -> (axum::http::StatusCode, Value) {
        use tower::ServiceExt as _;
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::PUT)
                    .uri(uri)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .expect("the request builds"),
            )
            .await
            .expect("the router responds");
        let status = response.status();
        (status, body_json(response).await)
    }

    /// The object's explicit `flow_object_grants` rows, in a stable order. Read from the table,
    /// not from a response body, so "the reply looked right" cannot cover for "the rows are
    /// wrong".
    async fn explicit_roster(state: &AppState, object_id: Uuid) -> Vec<(String, Uuid, String)> {
        #[derive(FromQueryResult)]
        struct Row {
            principal_kind: String,
            principal_id: Uuid,
            level: String,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT principal_kind, principal_id, level FROM flow_object_grants \
              WHERE object_id = $1 ORDER BY principal_kind, principal_id",
            vec![object_id.into()],
        ))
        .all(&state.db)
        .await
        .expect("query runs")
        .into_iter()
        .map(|row| (row.principal_kind, row.principal_id, row.level))
        .collect()
    }

    /// Creates a page and gives two bot principals an explicit grant each, both over HTTP.
    async fn page_with_two_grants(
        state: &AppState,
        app: &axum::Router,
        workspace_id: Uuid,
        owner_id: Uuid,
        first: Uuid,
        second: Uuid,
    ) -> Uuid {
        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Boundary Fixture".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id = Uuid::parse_str(create_body["data"]["object"]["id"].as_str().expect("object id"))
            .expect("object id is a uuid");

        let (status, body) = http_put(
            app,
            &format!("/api/v1/flow/objects/{object_id}/grants"),
            &json!({
                "grants": [
                    {"principal_kind": "bot", "principal_id": first, "level": "full_access"},
                    {"principal_kind": "bot", "principal_id": second, "level": "edit"},
                ],
                "idempotency_key": Uuid::new_v4().to_string(),
            })
            .to_string(),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(
            explicit_roster(state, object_id).await.len(),
            2,
            "fixture premise: two explicit grants exist before the boundary"
        );
        object_id
    }

    /// ★ The transport→domain seam of `PUT /api/v1/flow/objects/{object_id}/inheritance`.
    ///
    /// `ADR-0012` §4.1 point 2 makes `initial_grants` a whole-table **replacement**, and
    /// `rest-api-v1.md` spells the field `initial_grants?`. Those two together mean the wire has
    /// three distinct requests, and conflating the first two wipes an object's entire
    /// authorization roster on a request that only meant to flip a flag:
    ///
    /// | request body | meaning |
    /// |---|---|
    /// | no `initial_grants` key | leave every existing grant alone |
    /// | `"initial_grants": []` | clear every explicit grant |
    /// | `"initial_grants": [...]` | replace the roster with exactly this list |
    ///
    /// This test exists because the domain layer and the deserialization layer were each covered
    /// on their own while **the handler line that joins them was not**: a one-line change to
    /// `initial_grants: req.initial_grants.map(...)` — folding `None` into `Some(vec![])` —
    /// reintroduced the whole defect with every other test still green. Driving a real
    /// `http::Request` through a real `axum::Router` is what puts that line under test.
    #[tokio::test]
    async fn the_inheritance_route_keeps_an_absent_initial_grants_distinct_from_an_empty_one() {
        let scratch = scratch_or_skip!("inheritance_initial_grants");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let app = authorization_router(state.clone(), owner_id);

        let alice = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let carol = Uuid::new_v4();

        // (1) The key is absent: a pure boundary flip must not touch a single grant row.
        let absent = page_with_two_grants(&state, &app, workspace_id, owner_id, alice, bob).await;
        let before = explicit_roster(&state, absent).await;
        let (status, body) = http_put(
            &app,
            &format!("/api/v1/flow/objects/{absent}/inheritance"),
            &json!({ "inherit_from_parent": false, "idempotency_key": Uuid::new_v4().to_string() }).to_string(),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["inherit_from_parent"], false, "{body}");
        assert_eq!(
            explicit_roster(&state, absent).await,
            before,
            "a request that never mentioned `initial_grants` cleared the roster"
        );

        // (2) The key is present and empty: the explicit "clear it".
        let emptied = page_with_two_grants(&state, &app, workspace_id, owner_id, alice, bob).await;
        let (status, body) = http_put(
            &app,
            &format!("/api/v1/flow/objects/{emptied}/inheritance"),
            &json!({
                "inherit_from_parent": false,
                "initial_grants": [],
                "idempotency_key": Uuid::new_v4().to_string(),
            })
            .to_string(),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(
            explicit_roster(&state, emptied).await,
            Vec::new(),
            "`\"initial_grants\": []` must clear every explicit grant"
        );

        // (3) The key is present and non-empty: replacement, not merge.
        let replaced = page_with_two_grants(&state, &app, workspace_id, owner_id, alice, bob).await;
        let (status, body) = http_put(
            &app,
            &format!("/api/v1/flow/objects/{replaced}/inheritance"),
            &json!({
                "inherit_from_parent": false,
                "initial_grants": [{"principal_kind": "bot", "principal_id": carol, "level": "view"}],
                "idempotency_key": Uuid::new_v4().to_string(),
            })
            .to_string(),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(
            explicit_roster(&state, replaced).await,
            vec![("bot".to_string(), carol, "view".to_string())],
            "a non-empty `initial_grants` must replace the roster, not merge onto it"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn create_get_list_and_history_round_trip_against_a_real_database() {
        let scratch = scratch_or_skip!("roundtrip");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        // Every real HTTP response is HTTP 200; business status is `code` in the envelope.
        let create_response = to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "My First Page".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: Some("initial create".to_string()),
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        );
        assert_eq!(create_response.status(), axum::http::StatusCode::OK);
        let create_body = body_json(create_response).await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id = Uuid::parse_str(
            create_body["data"]["object"]["id"]
                .as_str()
                .expect("object id is a string"),
        )
        .expect("object id is a uuid");
        assert_eq!(create_body["data"]["object"]["title"], "My First Page");
        assert_eq!(create_body["data"]["object"]["object_type"], "page");
        assert_eq!(create_body["data"]["accepted_seq"], 0);
        assert!(create_body["data"]["event_id"].is_string());

        // GET the object back and see the same title/type the create returned.
        let get_response = to_response(
            get_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        );
        assert_eq!(get_response.status(), axum::http::StatusCode::OK);
        let get_body = body_json(get_response).await;
        assert_eq!(get_body["code"], 0, "{get_body}");
        assert_eq!(get_body["data"]["title"], "My First Page");
        assert_eq!(get_body["data"]["workspace_id"], workspace_id.to_string());

        // LIST returns the same object for its workspace.
        let list_response = to_response(
            list_flow_objects(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: None,
                    include_archived: false,
                }),
            )
            .await,
        );
        assert_eq!(list_response.status(), axum::http::StatusCode::OK);
        let list_body = body_json(list_response).await;
        assert_eq!(list_body["code"], 0, "{list_body}");
        let items = list_body["data"]["items"].as_array().expect("items is an array");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], object_id.to_string());

        // HISTORY is empty: this package writes no `collab_updates` row (no content commands).
        let history_response = to_response(
            get_flow_object_history(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(FlowObjectHistoryQuery {
                    before_seq: None,
                    limit: None,
                }),
            )
            .await,
        );
        assert_eq!(history_response.status(), axum::http::StatusCode::OK);
        let history_body = body_json(history_response).await;
        assert_eq!(history_body["code"], 0, "{history_body}");
        assert_eq!(
            history_body["data"]["items"]
                .as_array()
                .expect("items is an array")
                .len(),
            0
        );

        scratch.drop_self().await;
    }

    /// Migration 0054 defines `flow_object_projections.state` as the valid empty JSON object.
    /// Markdown rendering must interpret that persisted default as an empty semantic snapshot,
    /// not turn a legal row into an internal error.
    #[tokio::test]
    async fn markdown_render_accepts_the_empty_projection_state_default() {
        let scratch = scratch_or_skip!("markdown-empty-state");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "Empty state").await;
        exec(
            &state,
            "UPDATE flow_object_projections SET state = '{}'::jsonb WHERE object_id = $1",
            vec![object_id.into()],
        )
        .await;

        let response = body_json(to_response(
            get_flow_object(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: Some("markdown".to_string()),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(response["code"], 0, "{response}");
        assert_eq!(response["data"]["semantic_content"]["rendered"], "# Empty state\n");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn create_rejects_an_unregistered_object_type_via_body_code_not_http_status() {
        let scratch = scratch_or_skip!("bad-object-type");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let response = to_response(
            create_flow_object(
                State(state.clone()),
                claims,
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "not_a_real_type".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Doesn't matter".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        );

        // The error is `invalid_update`/`BadRequest`, carried entirely in the envelope: HTTP
        // status stays 200 and `code` is the business code, never the other way around.
        assert_eq!(
            response.status(),
            axum::http::StatusCode::OK,
            "errors must not change the transport status code"
        );
        let body = body_json(response).await;
        assert_eq!(body["code"], 400, "{body}");
        assert!(body["data"].is_null(), "{body}");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn create_on_a_workspace_without_flow_enabled_is_forbidden_via_body_code() {
        let scratch = scratch_or_skip!("flow-disabled");
        let state = state_for(scratch.db.clone());
        // `flow_enabled = false`: the row exists (unlike a never-provisioned workspace) but the
        // rollout flag is off, which must fail exactly like a missing row (fail closed).
        let (workspace_id, owner_id) = seed_workspace(&state, false).await;
        let claims = claims_for(owner_id);

        let response = to_response(
            create_flow_object(
                State(state.clone()),
                claims,
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Should never be created".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        );

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 403, "{body}");
        assert!(body["data"].is_null(), "{body}");

        // Nothing was written: `feature_disabled` must reject before any flow_objects insert.
        let count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_objects WHERE workspace_id = $1 AND object_type = 'page'",
                vec![workspace_id.into()],
            ))
            .await
            .expect("count query runs")
            .expect("count query returns a row");
        let n: i64 = count.try_get("", "n").expect("count column reads");
        assert_eq!(n, 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn get_unknown_object_is_not_found_via_body_code() {
        let scratch = scratch_or_skip!("not-found");
        let state = state_for(scratch.db.clone());
        let (_workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let response = to_response(
            get_flow_object(
                State(state.clone()),
                claims,
                None,
                Path(Uuid::new_v4()),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        );

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 404, "{body}");
        assert!(body["data"].is_null(), "{body}");

        scratch.drop_self().await;
    }

    /// All four v0.5 read paths share effective object authorization. The restricted page has a
    /// boundary and no member grant, so the DB oracle is `Denied`: it must disappear from the list
    /// and every object-id read must collapse to the same `not_found` envelope.
    #[tokio::test]
    async fn effective_permission_filters_every_flow_read_path_without_count_leakage() {
        let scratch = scratch_or_skip!("effective_read_filter");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let visible_id = create_page_as_owner(&state, workspace_id, owner_id, "Visible page").await;
        let restricted_id = create_page_as_owner(&state, workspace_id, owner_id, "Restricted page").await;
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![restricted_id.into()],
        )
        .await;
        exec(
            &state,
            "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id = $1",
            vec![workspace_id.into()],
        )
        .await;

        let list = body_json(to_response(
            list_flow_objects(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: Some(50),
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(list["code"], 0, "{list}");
        let items = list["data"]["items"].as_array().expect("items is an array");
        assert_eq!(
            items.len(),
            1,
            "the hidden candidate must not affect response cardinality"
        );
        assert_eq!(items[0]["id"], visible_id.to_string());
        let response_keys: std::collections::BTreeSet<&str> = list["data"]
            .as_object()
            .expect("list data is an object")
            .keys()
            .map(String::as_str)
            .collect();
        let allowed_keys = ["items", "next_cursor"].into_iter().collect();
        assert!(
            response_keys.is_subset(&allowed_keys),
            "the list response exposed a key outside the frozen whitelist: {response_keys:?}"
        );
        assert!(
            response_keys.contains("items"),
            "the required items key is missing: {list}"
        );

        let get = body_json(to_response(
            get_flow_object(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(restricted_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        let bootstrap = body_json(to_response(
            get_flow_object_bootstrap(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(restricted_id),
                Query(GetFlowObjectBootstrapQuery {
                    known_seq: None,
                    known_frontier: None,
                }),
            )
            .await,
        ))
        .await;
        let history = body_json(to_response(
            get_flow_object_history(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(restricted_id),
                Query(FlowObjectHistoryQuery {
                    before_seq: None,
                    limit: None,
                }),
            )
            .await,
        ))
        .await;
        for body in [&get, &bootstrap, &history] {
            assert_eq!(body["code"], 404, "{body}");
            assert!(body["data"].is_null(), "{body}");
        }

        scratch.drop_self().await;
    }

    /// A stale high-privilege cache entry is adversarial input, not authority. The read must miss
    /// through to the database, and the write path must ignore the cache altogether.
    #[tokio::test]
    async fn poisoned_high_privilege_cache_cannot_make_read_visible_or_write_persist() {
        let scratch = scratch_or_skip!("poisoned_cache_read_write");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "Restricted").await;
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        exec(
            &state,
            "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id = $1",
            vec![workspace_id.into()],
        )
        .await;
        let current_epoch = read_epoch(&state, workspace_id).await;
        assert!(current_epoch > 0, "the poison must be stale by construction");
        let cache = PermissionCache::for_state(&state).expect("permission cache is available");
        cache.put_for_test(
            workspace_id,
            PrincipalKind::User,
            member_id,
            object_id,
            PermissionLevel::FullAccess,
            current_epoch - 1,
        );

        let hidden = body_json(to_response(
            get_flow_object(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(object_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            hidden["code"], 404,
            "the stale full_access entry made a denied object visible: {hidden}"
        );

        cache.put_for_test(
            workspace_id,
            PrincipalKind::User,
            member_id,
            object_id,
            PermissionLevel::FullAccess,
            current_epoch - 1,
        );
        let head_before = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT cd.head_seq FROM collab_documents cd WHERE cd.object_id = $1",
                vec![object_id.into()],
            ))
            .await
            .expect("head query runs")
            .expect("document exists")
            .try_get::<i64>("", "head_seq")
            .expect("head_seq reads");
        let rejected = body_json(to_response(
            post_flow_object_command(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(object_id),
                Json(ExecuteFlowCommandRequest {
                    command: FlowCommandEnvelope {
                        command_type: "set_title".to_string(),
                        payload: json!({"title": "Must not persist"}),
                    },
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                }),
            )
            .await,
        ))
        .await;
        assert_ne!(rejected["code"], 0, "the poisoned cache authorized a write: {rejected}");
        let head_after = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT cd.head_seq FROM collab_documents cd WHERE cd.object_id = $1",
                vec![object_id.into()],
            ))
            .await
            .expect("head query runs")
            .expect("document exists")
            .try_get::<i64>("", "head_seq")
            .expect("head_seq reads");
        assert_eq!(head_after, head_before, "a denied write advanced the canonical head");

        let owner_write = body_json(to_response(
            post_flow_object_command(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_id),
                Json(ExecuteFlowCommandRequest {
                    command: FlowCommandEnvelope {
                        command_type: "set_title".to_string(),
                        payload: json!({"title": "Owner persists"}),
                    },
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            owner_write["code"], 0,
            "the positive write control failed: {owner_write}"
        );

        scratch.drop_self().await;
    }

    async fn run_depth_32_content_command(
        state: &AppState,
        member_id: Uuid,
        leaf: Uuid,
        sample: usize,
    ) -> (Value, f64) {
        let started = std::time::Instant::now();
        let body = body_json(to_response(
            post_flow_object_command(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(leaf),
                Json(ExecuteFlowCommandRequest {
                    command: FlowCommandEnvelope {
                        command_type: "set_title".to_string(),
                        payload: json!({"title": format!("Depth 32 sample {sample}")}),
                    },
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                }),
            )
            .await,
        ))
        .await;
        (body, started.elapsed().as_secs_f64() * 1000.0)
    }

    fn measured_p95_and_max(samples: &mut [f64]) -> (f64, f64) {
        samples.sort_by(f64::total_cmp);
        let p95_index = ((samples.len() * 95).div_ceil(100)).saturating_sub(1);
        (samples[p95_index], *samples.last().expect("samples are non-empty"))
    }

    const DEPTH_32_WARMUP_ROUNDS: usize = 5;
    const DEPTH_32_MEASURED_ROUNDS: usize = 30;
    const DEPTH_32_MIN_SAMPLES: usize = 30;
    const FROZEN_LOCK_HOLD_P95_MS_MAX: f64 = 25.0;
    const FROZEN_LOCK_HOLD_SINGLE_MS_MAX: f64 = 100.0;

    /// Ordinary debug/parallel runs are suitable for detecting an order-of-magnitude regression,
    /// not for judging the production lock budget. These deliberately non-contractual ceilings are
    /// ten times the frozen production limits and are labelled diagnostic in every emitted record.
    const DIAGNOSTIC_LOCK_HOLD_P95_MS_MAX: f64 = FROZEN_LOCK_HOLD_P95_MS_MAX * 10.0;
    const DIAGNOSTIC_LOCK_HOLD_SINGLE_MS_MAX: f64 = FROZEN_LOCK_HOLD_SINGLE_MS_MAX * 10.0;

    /// The inheritance evaluator is a small, separately measured recursive query rather than the
    /// whole locked transaction. It stayed below 1 ms p95 in both parallel full-suite observations;
    /// these regression ceilings retain roughly tenfold p95 headroom without claiming to be a
    /// frozen production SLO.
    const EVALUATION_REGRESSION_P95_MS_MAX: f64 = 10.0;
    const EVALUATION_REGRESSION_SINGLE_MS_MAX: f64 = 25.0;

    const DEDICATED_PG_CONTAINER_ENV: &str = "OPENPR_FLOW_DEDICATED_PG_CONTAINER";
    const QUIET_PG_QUALIFIED_ENV: &str = "OPENPR_FLOW_QUIET_PG_QUALIFIED";

    struct Depth32Measurements {
        evaluation_samples_ms: Vec<f64>,
        lock_hold_samples_ms: Vec<f64>,
        end_to_end_samples_ms: Vec<f64>,
        injected_delay_millis: u64,
    }

    struct Depth32Metrics {
        lock_hold_p95_ms: f64,
        lock_hold_max_ms: f64,
    }

    #[derive(Clone, Copy)]
    enum Depth32MeasurementCondition {
        OrdinaryParallelRegression,
        OfficialRelease,
    }

    impl Depth32MeasurementCondition {
        const fn as_str(self) -> &'static str {
            match self {
                Self::OrdinaryParallelRegression => "ordinary_parallel_regression",
                Self::OfficialRelease => "official_release_dedicated_quiet",
            }
        }

        const fn frozen_budget_status(self) -> &'static str {
            match self {
                Self::OrdinaryParallelRegression => "skipped_invalid_measurement_condition",
                Self::OfficialRelease => "executed",
            }
        }
    }

    const fn depth_32_build_profile() -> &'static str {
        if cfg!(debug_assertions) { "debug" } else { "release" }
    }

    fn official_depth_32_environment_problem() -> Option<String> {
        if cfg!(debug_assertions) {
            return Some("a release build is required (`cargo test --release`)".to_string());
        }
        if std::env::var(TEST_DATABASE_URL_ENV).unwrap_or_default().is_empty() {
            return Some(format!("{TEST_DATABASE_URL_ENV} is required for real PostgreSQL"));
        }
        let dedicated = std::env::var(DEDICATED_PG_CONTAINER_ENV).unwrap_or_default();
        if dedicated.trim().is_empty() {
            return Some(format!(
                "{DEDICATED_PG_CONTAINER_ENV} must declare the dedicated PostgreSQL instance"
            ));
        }
        if std::env::var(QUIET_PG_QUALIFIED_ENV).as_deref() != Ok("1") {
            return Some(format!(
                "{QUIET_PG_QUALIFIED_ENV}=1 must attest that no concurrent workload is using the declared instance"
            ));
        }
        None
    }

    /// Runs exactly the same depth-32 fixture and probe scopes for the ordinary regression test and
    /// the explicit official measurement. Keeping one producer prevents the two conditions from
    /// drifting to different paths while leaving the 1d7f8d1 task-local probe isolation untouched.
    async fn measure_depth_32_content_commit_path(label: &str) -> Option<Depth32Measurements> {
        let scratch = scratch(label).await?;
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let leaf = create_page_as_owner(&state, workspace_id, owner_id, "Depth 32 leaf").await;

        let navigator_root = crate::flow::repository::fetch_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("root lookup runs")
            .expect("creating the leaf materializes the workspace root");
        let mut parent = Some(navigator_root);
        let mut root = None;
        for index in 0..32 {
            let id = Uuid::new_v4();
            exec(
                &state,
                "INSERT INTO flow_objects (id, workspace_id, object_type, parent_id, inherit_from_parent) \
                 VALUES ($1, $2, 'page', $3, true)",
                vec![id.into(), workspace_id.into(), parent.into()],
            )
            .await;
            if index == 0 {
                root = Some(id);
            }
            parent = Some(id);
        }
        exec(
            &state,
            "UPDATE flow_objects SET parent_id = $1 WHERE id = $2",
            vec![parent.into(), leaf.into()],
        )
        .await;
        let root = root.expect("root exists");
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![root.into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'user', $3, 'full_access')",
            vec![workspace_id.into(), root.into(), member_id.into()],
        )
        .await;
        let chain = crate::flow::collab::authz::inheritance_chain(&state.db, workspace_id, leaf)
            .await
            .expect("the exact depth-32 chain is complete");
        assert_eq!(
            chain.ids.len(),
            34,
            "the hidden navigator root plus visible depths zero through 32"
        );

        let document_id = document_id_for(&state, leaf).await;
        let injected_delay_ms = std::env::var("OPENPR_TEST_AUTHZ_DEPTH32_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let evaluation_probe =
            crate::flow::collab::authz::EvaluationProbe::new(leaf, Duration::from_millis(injected_delay_ms));
        let locked_phase_probe = crate::flow::collab::write::LockedPhaseProbe::new(document_id);
        let (evaluation_samples_ms, lock_hold_samples_ms, end_to_end_samples_ms) =
            Box::pin(evaluation_probe.scope(locked_phase_probe.scope(async {
                for sample in 0..DEPTH_32_WARMUP_ROUNDS {
                    let (body, _) = run_depth_32_content_command(&state, member_id, leaf, sample).await;
                    assert_eq!(body["code"], 0, "the warmup depth-32 command failed: {body}");
                }
                let warmup_evaluation_samples = evaluation_probe.take_samples();
                let warmup_lock_hold_samples = locked_phase_probe.take_samples();
                assert_eq!(warmup_evaluation_samples.len(), DEPTH_32_WARMUP_ROUNDS);
                assert_eq!(warmup_lock_hold_samples.len(), DEPTH_32_WARMUP_ROUNDS);

                let mut end_to_end_samples_ms = Vec::with_capacity(DEPTH_32_MEASURED_ROUNDS);
                for sample in DEPTH_32_WARMUP_ROUNDS..DEPTH_32_WARMUP_ROUNDS + DEPTH_32_MEASURED_ROUNDS {
                    let (body, elapsed_ms) = run_depth_32_content_command(&state, member_id, leaf, sample).await;
                    assert_eq!(body["code"], 0, "the measured depth-32 command failed: {body}");
                    end_to_end_samples_ms.push(elapsed_ms);
                }

                (
                    evaluation_probe.take_samples(),
                    locked_phase_probe.take_samples(),
                    end_to_end_samples_ms,
                )
            })))
            .await;
        scratch.drop_self().await;

        Some(Depth32Measurements {
            evaluation_samples_ms,
            lock_hold_samples_ms,
            end_to_end_samples_ms,
            injected_delay_millis: injected_delay_ms,
        })
    }

    fn assess_depth_32_measurements(
        measurements: Depth32Measurements,
        condition: Depth32MeasurementCondition,
    ) -> Depth32Metrics {
        let Depth32Measurements {
            mut evaluation_samples_ms,
            mut lock_hold_samples_ms,
            mut end_to_end_samples_ms,
            injected_delay_millis,
        } = measurements;

        // Sample sufficiency is decided before a percentile is calculated. In particular, a
        // short vector can never turn its maximum into a plausible-looking p95 and pass.
        assert!(
            evaluation_samples_ms.len() >= DEPTH_32_MIN_SAMPLES,
            "depth-32 inheritance evaluation produced only {} samples; at least {DEPTH_32_MIN_SAMPLES} are required",
            evaluation_samples_ms.len()
        );
        assert!(
            lock_hold_samples_ms.len() >= DEPTH_32_MIN_SAMPLES,
            "depth-32 lock hold produced only {} samples; at least {DEPTH_32_MIN_SAMPLES} are required",
            lock_hold_samples_ms.len()
        );
        assert_eq!(evaluation_samples_ms.len(), DEPTH_32_MEASURED_ROUNDS);
        assert_eq!(lock_hold_samples_ms.len(), DEPTH_32_MEASURED_ROUNDS);
        assert_eq!(end_to_end_samples_ms.len(), DEPTH_32_MEASURED_ROUNDS);

        let (evaluation_p95_ms, evaluation_max_ms) = measured_p95_and_max(&mut evaluation_samples_ms);
        let (lock_hold_p95_ms, lock_hold_max_ms) = measured_p95_and_max(&mut lock_hold_samples_ms);
        let (end_to_end_p95_ms, end_to_end_max_ms) = measured_p95_and_max(&mut end_to_end_samples_ms);
        eprintln!(
            "AUTHZ_DEPTH32_COMMIT_BUDGET_EVIDENCE {}",
            json!({
                "depth": 32,
                "boundary_depth": 32,
                "build_profile": depth_32_build_profile(),
                "measurement_condition": condition.as_str(),
                "frozen_lock_budget_status": condition.frozen_budget_status(),
                "frozen_lock_hold_p95_ms_max": FROZEN_LOCK_HOLD_P95_MS_MAX,
                "frozen_lock_hold_single_ms_max": FROZEN_LOCK_HOLD_SINGLE_MS_MAX,
                "warmup_rounds": DEPTH_32_WARMUP_ROUNDS,
                "minimum_samples": DEPTH_32_MIN_SAMPLES,
                "samples": lock_hold_samples_ms.len(),
                "p95_ms": lock_hold_p95_ms,
                "max_ms": lock_hold_max_ms,
                "lock_hold_ms_p95": lock_hold_p95_ms,
                "lock_hold_ms_max": lock_hold_max_ms,
                "lock_hold_samples_ms": lock_hold_samples_ms,
                "lock_hold_diagnostic_only_p95_ms_max": DIAGNOSTIC_LOCK_HOLD_P95_MS_MAX,
                "lock_hold_diagnostic_only_single_ms_max": DIAGNOSTIC_LOCK_HOLD_SINGLE_MS_MAX,
                "inheritance_evaluation_samples": evaluation_samples_ms.len(),
                "inheritance_evaluation_ms_p95": evaluation_p95_ms,
                "inheritance_evaluation_ms_max": evaluation_max_ms,
                "inheritance_evaluation_samples_ms": evaluation_samples_ms,
                "inheritance_evaluation_regression_p95_ms_max": EVALUATION_REGRESSION_P95_MS_MAX,
                "inheritance_evaluation_regression_single_ms_max": EVALUATION_REGRESSION_SINGLE_MS_MAX,
                "end_to_end_samples": end_to_end_samples_ms.len(),
                "end_to_end_ms_p95_diagnostic_only": end_to_end_p95_ms,
                "end_to_end_ms_max_diagnostic_only": end_to_end_max_ms,
                "end_to_end_samples_ms_diagnostic_only": end_to_end_samples_ms,
                "end_to_end_budget_ms": Value::Null,
                "end_to_end_budget_status": "not_frozen_for_sequential_in_process_rest_handler",
                "injected_authz_delay_ms": injected_delay_millis,
                "measurement_scope": "depth-32 effective_permission plus write::run_locked_phase BEGIN-to-COMMIT",
            })
        );
        assert!(
            evaluation_p95_ms <= EVALUATION_REGRESSION_P95_MS_MAX,
            "depth-32 inheritance-evaluation p95 {evaluation_p95_ms:.3}ms exceeded the non-contractual regression ceiling {EVALUATION_REGRESSION_P95_MS_MAX}ms"
        );
        assert!(
            evaluation_max_ms <= EVALUATION_REGRESSION_SINGLE_MS_MAX,
            "depth-32 inheritance-evaluation max {evaluation_max_ms:.3}ms exceeded the non-contractual regression ceiling {EVALUATION_REGRESSION_SINGLE_MS_MAX}ms"
        );
        assert!(
            lock_hold_p95_ms <= DIAGNOSTIC_LOCK_HOLD_P95_MS_MAX,
            "diagnostic only: depth-32 lock-hold p95 {lock_hold_p95_ms:.3}ms exceeded the 10x regression ceiling {DIAGNOSTIC_LOCK_HOLD_P95_MS_MAX}ms"
        );
        assert!(
            lock_hold_max_ms <= DIAGNOSTIC_LOCK_HOLD_SINGLE_MS_MAX,
            "diagnostic only: depth-32 lock-hold max {lock_hold_max_ms:.3}ms exceeded the 10x regression ceiling {DIAGNOSTIC_LOCK_HOLD_SINGLE_MS_MAX}ms"
        );

        Depth32Metrics {
            lock_hold_p95_ms,
            lock_hold_max_ms,
        }
    }

    /// The ordinary default-parallel regression keeps testing the exact depth-32 path, exact
    /// task-local sample ownership, the inheritance evaluator, and gross lock-hold regressions. It
    /// deliberately does not judge the production 25/100 ms lock budget from an unoptimised process
    /// competing with the rest of the workspace suite.
    #[tokio::test]
    async fn depth_32_content_commit_path_preserves_samples_and_regression_bounds() {
        let Some(measurements) = measure_depth_32_content_commit_path("depth_32_parallel_regression").await else {
            eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
            return;
        };
        let _metrics =
            assess_depth_32_measurements(measurements, Depth32MeasurementCondition::OrdinaryParallelRegression);
        eprintln!(
            "skipped: depth-32 frozen 25/100 ms lock budget requires the ignored release/dedicated/quiet measurement; ordinary sample-ownership, evaluation, and diagnostic assertions executed"
        );
    }

    /// The frozen production lock budget is meaningful only under ADR-0010's official conditions:
    /// a release build, real `PostgreSQL` declared as the dedicated instance, an explicit quiet/no-
    /// concurrency attestation, the same locked fixture, five warmups, and thirty measurements.
    /// Cargo marks this test `ignored` in every ordinary suite, so absence of those conditions can
    /// never appear as a fast passing budget result. An explicit `--ignored` run fails closed when
    /// any declaration is missing.
    #[tokio::test]
    #[ignore = "skipped: frozen 25/100 ms budget requires release build plus declared dedicated and quiet PostgreSQL"]
    async fn depth_32_content_commit_path_stays_inside_the_frozen_authz_budgets() {
        if let Some(problem) = official_depth_32_environment_problem() {
            panic!("OFFICIAL DEPTH-32 ENVIRONMENT NOT SATISFIED: {problem}");
        }
        let measurements = measure_depth_32_content_commit_path("depth_32_official_budget")
            .await
            .expect("the declared real PostgreSQL instance must create a scratch database");
        let metrics = assess_depth_32_measurements(measurements, Depth32MeasurementCondition::OfficialRelease);
        assert!(
            metrics.lock_hold_p95_ms <= FROZEN_LOCK_HOLD_P95_MS_MAX,
            "official depth-32 lock-hold p95 {:.3}ms exceeded {FROZEN_LOCK_HOLD_P95_MS_MAX}ms",
            metrics.lock_hold_p95_ms
        );
        assert!(
            metrics.lock_hold_max_ms <= FROZEN_LOCK_HOLD_SINGLE_MS_MAX,
            "official depth-32 lock-hold max {:.3}ms exceeded {FROZEN_LOCK_HOLD_SINGLE_MS_MAX}ms",
            metrics.lock_hold_max_ms
        );
    }

    #[tokio::test]
    async fn foreign_and_absent_object_reads_collapse_to_the_same_answer() {
        let scratch = scratch_or_skip!("read_existence_collapse");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let (other_workspace_id, other_owner_id) = seed_workspace(&state, true).await;
        let foreign_id = create_page_as_owner(&state, other_workspace_id, other_owner_id, "Foreign page").await;

        let read = |object_id| {
            get_flow_object(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(object_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
        };
        let foreign = body_json(to_response(read(foreign_id).await)).await;
        let absent = body_json(to_response(read(Uuid::new_v4()).await)).await;
        assert_eq!(foreign["code"], 404, "{foreign}");
        assert_eq!(
            foreign, absent,
            "cross-tenant existence must not change the safe answer"
        );
        let _ = owner_id;

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn object_reads_reauthorize_after_their_final_epoch_check_changes() {
        let scratch = scratch_or_skip!("object_read_epoch_retry");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let object_ids = [
            create_page_as_owner(&state, workspace_id, owner_id, "Retry object").await,
            create_page_as_owner(&state, workspace_id, owner_id, "Retry bootstrap").await,
            create_page_as_owner(&state, workspace_id, owner_id, "Retry history").await,
        ];

        // On a cold cache, checks one and two surround DB authorization. Changing the epoch at
        // check three therefore targets the final pre-return fence in each query function.
        crate::flow::policy::plan_epoch_changes_for_test(workspace_id, 2, 1);
        let object = body_json(to_response(
            get_flow_object(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_ids[0]),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(object["code"], 0, "the object read did not recover: {object}");

        crate::flow::policy::plan_epoch_changes_for_test(workspace_id, 2, 1);
        let bootstrap = body_json(to_response(
            get_flow_object_bootstrap(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_ids[1]),
                Query(GetFlowObjectBootstrapQuery {
                    known_seq: None,
                    known_frontier: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(bootstrap["code"], 0, "the bootstrap read did not recover: {bootstrap}");

        crate::flow::policy::plan_epoch_changes_for_test(workspace_id, 2, 1);
        let history = body_json(to_response(
            get_flow_object_history(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_ids[2]),
                Query(FlowObjectHistoryQuery {
                    before_seq: None,
                    limit: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(history["code"], 0, "the history read did not recover: {history}");

        // Each member read succeeds on the workspace baseline, then its final epoch check applies
        // a real boundary revocation. A correct route discards the prepared response, retries, and
        // returns the same fieldless not-found envelope that a normally hidden object returns.
        for (index, object_id) in object_ids.into_iter().enumerate() {
            let visible = match index {
                0 => {
                    body_json(to_response(
                        get_flow_object(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(object_id),
                            Query(GetFlowObjectQuery {
                                at_seq: None,
                                render: None,
                            }),
                        )
                        .await,
                    ))
                    .await
                }
                1 => {
                    body_json(to_response(
                        get_flow_object_bootstrap(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(object_id),
                            Query(GetFlowObjectBootstrapQuery {
                                known_seq: None,
                                known_frontier: None,
                            }),
                        )
                        .await,
                    ))
                    .await
                }
                _ => {
                    body_json(to_response(
                        get_flow_object_history(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(object_id),
                            Query(FlowObjectHistoryQuery {
                                before_seq: None,
                                limit: None,
                            }),
                        )
                        .await,
                    ))
                    .await
                }
            };
            assert_eq!(
                visible["code"], 0,
                "the member must be authorized before revocation: {visible}"
            );

            // The successful member read above warmed this principal's permission cache, so this
            // request has one initial epoch check and then the final pre-return check.
            crate::flow::policy::plan_object_revocation_for_test(workspace_id, object_id, 1);
            let revoked = match index {
                0 => {
                    body_json(to_response(
                        get_flow_object(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(object_id),
                            Query(GetFlowObjectQuery {
                                at_seq: None,
                                render: None,
                            }),
                        )
                        .await,
                    ))
                    .await
                }
                1 => {
                    body_json(to_response(
                        get_flow_object_bootstrap(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(object_id),
                            Query(GetFlowObjectBootstrapQuery {
                                known_seq: None,
                                known_frontier: None,
                            }),
                        )
                        .await,
                    ))
                    .await
                }
                _ => {
                    body_json(to_response(
                        get_flow_object_history(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(object_id),
                            Query(FlowObjectHistoryQuery {
                                before_seq: None,
                                limit: None,
                            }),
                        )
                        .await,
                    ))
                    .await
                }
            };
            assert_eq!(
                revoked["code"], 404,
                "a read prepared before revocation escaped: {revoked}"
            );
            assert!(revoked["data"].is_null(), "a denied read exposed data: {revoked}");
        }

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn list_epoch_retry_is_bounded_to_three_complete_attempts() {
        let scratch = scratch_or_skip!("list_epoch_retry");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        create_page_as_owner(&state, workspace_id, owner_id, "Retry list").await;

        crate::flow::policy::plan_epoch_changes_for_test(workspace_id, 0, 2);
        let recovered = body_json(to_response(
            list_flow_objects(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: Some(50),
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            recovered["code"], 0,
            "the third stable attempt must succeed: {recovered}"
        );

        crate::flow::policy::plan_epoch_changes_for_test(workspace_id, 0, 3);
        let exhausted = body_json(to_response(
            list_flow_objects(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: Some(50),
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            exhausted["code"], 409,
            "three unstable attempts must report conflict: {exhausted}"
        );
        assert_eq!(exhausted["error_code"], "authorization_churn");
        assert_eq!(
            exhausted["details"]["retry_after_ms"],
            crate::flow::policy::AUTHORIZATION_CHURN_RETRY_AFTER_MS
        );
        assert_eq!(
            exhausted["message"], "authorization changed repeatedly while the read was being evaluated",
            "the caller-safe explanation remains stable"
        );
        let forbidden = body_json(to_response::<Response>(Err(ApiError::typed(
            ApiErrorKind::Forbidden,
            "authorization changed repeatedly while the read was being evaluated",
        ))))
        .await;
        assert_eq!(forbidden["code"], 403);
        assert_eq!(forbidden["error_code"], "forbidden");
        assert_ne!(
            exhausted["error_code"], forbidden["error_code"],
            "authorization churn must be distinguishable from a real denial without reading message prose"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn grant_and_inheritance_endpoints_collapse_foreign_and_absent_objects() {
        let scratch = scratch_or_skip!("grant_existence_collapse");
        let state = state_for(scratch.db.clone());
        let (_workspace_id, owner_id) = seed_workspace(&state, true).await;
        let (foreign_workspace_id, foreign_owner_id) = seed_workspace(&state, true).await;
        let foreign_id = create_page_as_owner(
            &state,
            foreign_workspace_id,
            foreign_owner_id,
            "Foreign authorization object",
        )
        .await;
        let absent_id = Uuid::new_v4();

        let foreign_get = body_json(to_response(
            get_flow_object_grants(State(state.clone()), claims_for(owner_id), None, Path(foreign_id)).await,
        ))
        .await;
        let absent_get = body_json(to_response(
            get_flow_object_grants(State(state.clone()), claims_for(owner_id), None, Path(absent_id)).await,
        ))
        .await;
        assert_eq!(foreign_get["code"], 404, "{foreign_get}");
        assert_eq!(foreign_get, absent_get, "GET /grants leaked cross-tenant existence");

        let grants_request = || SetGrantsRequest {
            grants: Vec::new(),
            confirm_self_lockout: false,
            dry_run: false,
            idempotency_key: "grant-collapse-key".to_string(),
        };
        let foreign_put_grants = body_json(to_response(
            put_flow_object_grants(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(foreign_id),
                Json(grants_request()),
            )
            .await,
        ))
        .await;
        let absent_put_grants = body_json(to_response(
            put_flow_object_grants(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(absent_id),
                Json(grants_request()),
            )
            .await,
        ))
        .await;
        assert_eq!(foreign_put_grants["code"], 404, "{foreign_put_grants}");
        assert_eq!(
            foreign_put_grants, absent_put_grants,
            "PUT /grants leaked cross-tenant existence"
        );

        let inheritance_request = || SetInheritanceRequest {
            inherit_from_parent: false,
            confirm_self_lockout: false,
            dry_run: false,
            initial_grants: None,
            idempotency_key: "inheritance-collapse-key".to_string(),
        };
        let foreign_put_inheritance = body_json(to_response(
            put_flow_object_inheritance(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(foreign_id),
                Json(inheritance_request()),
            )
            .await,
        ))
        .await;
        let absent_put_inheritance = body_json(to_response(
            put_flow_object_inheritance(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(absent_id),
                Json(inheritance_request()),
            )
            .await,
        ))
        .await;
        assert_eq!(foreign_put_inheritance["code"], 404, "{foreign_put_inheritance}");
        assert_eq!(
            foreign_put_inheritance, absent_put_inheritance,
            "PUT /inheritance leaked cross-tenant existence"
        );

        scratch.drop_self().await;
    }

    /// 1,001 hidden rows force the overfetch loop one row past the public scan ceiling. The error
    /// must reject rather than return a misleading empty short page, and both numeric fields must
    /// expose only the fixed ceiling, never the actual pre-filter count.
    #[tokio::test]
    async fn authorized_overfetch_rejects_past_scan_budget_without_revealing_examined_rows() {
        let scratch = scratch_or_skip!("authorized_scan_budget");
        let state = state_for(scratch.db.clone());
        let (workspace_id, _owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let root_id = crate::flow::repository::ensure_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("scan fixture root materializes");
        exec(
            &state,
            "WITH objects AS ( \
                 INSERT INTO flow_objects (id, workspace_id, object_type, parent_id, inherit_from_parent, created_at) \
                 SELECT gen_random_uuid(), $1, 'page', $2, false, now() + n * interval '1 microsecond' \
                   FROM generate_series(1, 1001) AS n RETURNING id \
             ), documents AS ( \
                 INSERT INTO collab_documents (object_id, format_version, snapshot, snapshot_frontier, head_frontier) \
                 SELECT id, 'loro-1', '\\x'::bytea, '\\x'::bytea, '\\x'::bytea FROM objects \
             ) \
             INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, state, plain_text) \
             SELECT id, 0, '\\x'::bytea, 'hidden', '{}'::jsonb, '' FROM objects",
            vec![workspace_id.into(), root_id.into()],
        )
        .await;

        let body = body_json(to_response(
            list_flow_objects(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: Some(50),
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(body["error_code"], "limit_exceeded", "{body}");
        assert_eq!(body["details"]["limit_kind"], "scan_budget", "{body}");
        assert_eq!(body["details"]["limit"], 1000, "{body}");
        assert_eq!(body["details"]["observed"], 1000, "{body}");
        assert!(body["data"].is_null(), "a partial empty page must not escape: {body}");

        scratch.drop_self().await;
    }

    /// Exercises the audit's concrete interference shape: a maximum-size permission cache, a
    /// list that must inspect past the 1,000-row scan budget, and an unrelated content write in
    /// flight at the same time. The timings are diagnostic evidence rather than a brittle CI
    /// threshold; the assertions pin that both real operations executed to their intended ends.
    #[tokio::test]
    async fn full_scan_budget_and_content_write_complete_concurrently_with_a_full_cache() {
        let scratch = scratch_or_skip!("cache_list_write_concurrency");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let writable_id = create_page_as_owner(&state, workspace_id, owner_id, "Concurrent write target").await;
        let root_id = crate::flow::repository::fetch_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("root lookup runs")
            .expect("creating the writable page materializes the root");
        exec(
            &state,
            "WITH objects AS ( \
                 INSERT INTO flow_objects (id, workspace_id, object_type, parent_id, inherit_from_parent, created_at) \
                 SELECT gen_random_uuid(), $1, 'page', $2, false, now() + n * interval '1 microsecond' \
                   FROM generate_series(1, 1001) AS n RETURNING id \
             ), documents AS ( \
                 INSERT INTO collab_documents (object_id, format_version, snapshot, snapshot_frontier, head_frontier) \
                 SELECT id, 'loro-1', '\\x'::bytea, '\\x'::bytea, '\\x'::bytea FROM objects \
             ) \
             INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, state, plain_text) \
             SELECT id, 0, '\\x'::bytea, 'hidden', '{}'::jsonb, '' FROM objects",
            vec![workspace_id.into(), root_id.into()],
        )
        .await;

        let cache = PermissionCache::for_state(&state).expect("permission cache is available");
        for ordinal in 0_u128..20_000 {
            cache.put_for_test(
                Uuid::from_u128(1),
                PrincipalKind::User,
                Uuid::from_u128(2),
                Uuid::from_u128(ordinal + 100),
                PermissionLevel::View,
                1,
            );
        }
        assert_eq!(cache.len_for_test(), 20_000, "the measurement requires a full cache");

        let list_state = state.clone();
        let list_task = tokio::spawn(async move {
            let started = std::time::Instant::now();
            let body = body_json(to_response(
                list_flow_objects(
                    State(list_state),
                    claims_for(member_id),
                    None,
                    Path(workspace_id),
                    Query(ListFlowObjectsQuery {
                        project_id: None,
                        unprojected: false,
                        object_type: None,
                        parent_id: None,
                        q: None,
                        cursor: None,
                        limit: Some(50),
                        include_archived: false,
                    }),
                )
                .await,
            ))
            .await;
            (body, started.elapsed())
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        let write_started = std::time::Instant::now();
        let write_body = run_command(
            &state,
            &claims_for(owner_id),
            writable_id,
            "set_title",
            json!({"title": "Concurrent write completed"}),
        )
        .await;
        let write_elapsed = write_started.elapsed();
        let (list_body, list_elapsed) = list_task.await.expect("concurrent list task joins");

        assert_eq!(list_body["error_code"], "limit_exceeded", "{list_body}");
        assert_eq!(list_body["details"]["limit_kind"], "scan_budget", "{list_body}");
        assert_eq!(write_body["code"], 0, "{write_body}");
        assert!(
            !list_elapsed.is_zero() && !write_elapsed.is_zero(),
            "both concurrent wall-clock measurements must be recorded: list={list_elapsed:?}, write={write_elapsed:?}"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn create_idempotency_replays_the_exact_body_and_rejects_every_body_field_drift() {
        let scratch = scratch_or_skip!("idempotent-replay");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let idempotency_key = Uuid::new_v4().to_string();

        let request = || CreateFlowObjectRequest {
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Replayed Page".to_string(),
            idempotency_key: idempotency_key.clone(),
            message: None,
            initial_fields: Vec::new(),
            initial_view: None,
        };

        let first = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(request()),
            )
            .await,
        ))
        .await;
        assert_eq!(first["code"], 0, "{first}");
        let first_id = first["data"]["object"]["id"].clone();

        let second = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(request()),
            )
            .await,
        ))
        .await;
        assert_eq!(second["code"], 0, "{second}");
        assert_eq!(
            second["data"]["object"]["id"], first_id,
            "replay must return the original object id"
        );

        for (field, drifted) in [
            (
                "type",
                CreateFlowObjectRequest {
                    object_type: "navigator".to_string(),
                    ..request()
                },
            ),
            (
                "project_id",
                CreateFlowObjectRequest {
                    project_id: Some(Uuid::new_v4()),
                    ..request()
                },
            ),
            (
                "parent_id",
                CreateFlowObjectRequest {
                    parent_object_id: Some(Uuid::new_v4()),
                    ..request()
                },
            ),
            (
                "title",
                CreateFlowObjectRequest {
                    title: "Different title".to_string(),
                    ..request()
                },
            ),
            (
                "message",
                CreateFlowObjectRequest {
                    message: Some("different message".to_string()),
                    ..request()
                },
            ),
        ] {
            let body = body_json(to_response(
                create_flow_object(
                    State(state.clone()),
                    claims.clone(),
                    None,
                    Path(workspace_id),
                    Json(drifted),
                )
                .await,
            ))
            .await;
            assert_eq!(body["code"], 409, "{field} drift reused the original receipt: {body}");
            assert_eq!(
                body["message"], "idempotency_key was already used with a different create request body",
                "{field} drift returned the wrong conflict: {body}"
            );
        }

        // Only one caller-created Page was ever written, not two. The materialized navigator root
        // is a separate system aggregate and is intentionally excluded from this idempotency
        // assertion.
        let count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_objects WHERE workspace_id = $1 AND object_type = 'page'",
                vec![workspace_id.into()],
            ))
            .await
            .expect("count query runs")
            .expect("count query returns a row");
        let n: i64 = count.try_get("", "n").expect("count column reads");
        assert_eq!(n, 1);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn navigator_response_exposes_the_materialized_root_and_default_parent() {
        let scratch = scratch_or_skip!("navigator-root-response");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let created = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Top-level Page".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(created["code"], 0, "{created}");
        let page_id = created["data"]["object"]["id"].as_str().expect("page id").to_string();
        let root_id = created["data"]["object"]["parent_id"]
            .as_str()
            .expect("NR-3 default parent")
            .to_string();
        assert_ne!(page_id, root_id, "top-level Page must not itself be the root");

        let navigator = body_json(to_response(
            get_flow_navigator(
                State(state.clone()),
                claims,
                None,
                Path(workspace_id),
                Query(GetFlowNavigatorQuery {
                    project_id: None,
                    depth: Some(20),
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(navigator["code"], 0, "{navigator}");
        assert_eq!(navigator["data"]["root_object_id"], root_id, "{navigator}");
        assert_eq!(
            navigator["data"]["nodes"].as_array().map(Vec::len),
            Some(1),
            "{navigator}"
        );
        assert_eq!(navigator["data"]["nodes"][0]["object_id"], page_id, "{navigator}");
        assert_eq!(navigator["data"]["nodes"][0]["parent_id"], root_id, "{navigator}");
        assert!(navigator["data"]["nodes"][0]["position"].is_string(), "{navigator}");
        assert_eq!(navigator["data"]["nodes"][0]["type"], "page", "{navigator}");
        assert_eq!(navigator["data"]["document_seq"], 0, "{navigator}");
        assert!(navigator["data"]["frontier"].is_string(), "{navigator}");

        scratch.drop_self().await;
    }

    /// ADR-0018 NR-1: omitting `parent_object_id` in a project scope attaches the object to that
    /// project's own navigator root. It must not fall back to the unprojected workspace root.
    #[tokio::test]
    async fn project_scoped_create_without_parent_uses_only_the_project_navigator_root() {
        #[derive(FromQueryResult)]
        struct ScopeRow {
            project_id: Option<Uuid>,
            parent_id: Option<Uuid>,
            is_system_root: bool,
        }

        let scratch = scratch_or_skip!("project-default-parent");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'PRJ', 'Project root test', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;

        let response = to_response(
            create_flow_object(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: Some(project_id),
                    parent_object_id: None,
                    title: "Project top-level page".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        );
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 0, "{body}");
        let page_id =
            Uuid::parse_str(body["data"]["object"]["id"].as_str().expect("page id")).expect("page id is a uuid");
        let parent_id = Uuid::parse_str(body["data"]["object"]["parent_id"].as_str().expect("default parent id"))
            .expect("parent id is a uuid");

        let page = ScopeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id, parent_id, false AS is_system_root \
               FROM flow_objects WHERE id = $1",
            vec![page_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("page lookup runs")
        .expect("page exists");
        let root = ScopeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id, parent_id, \
                    flow_is_system_navigator_root(object_type, parent_id, governance_metadata) \
                        AS is_system_root \
               FROM flow_objects WHERE id = $1",
            vec![parent_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("root lookup runs")
        .expect("root exists");
        let workspace_root = crate::flow::repository::fetch_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("workspace root lookup runs")
            .expect("workspace root exists");

        assert_eq!(page.project_id, Some(project_id));
        assert_eq!(page.parent_id, Some(parent_id));
        assert_eq!(root.project_id, Some(project_id));
        assert_eq!(root.parent_id, None);
        assert!(root.is_system_root, "the lazy-created project root must be marked");
        assert_ne!(
            parent_id, workspace_root,
            "project create fell back to the NULL scope root"
        );

        // Reproduce a pre-0059 adopted root, then replay the migration. The public
        // `require_current` search must stay usable after adoption; a shape-only root predicate
        // would leave this workspace permanently stale because the root itself is never indexed.
        state
            .db
            .execute_unprepared(
                "ALTER TABLE flow_objects DROP CONSTRAINT flow_objects_navigator_root_role_check; \
                 DROP TRIGGER flow_objects_mark_system_navigator_root ON flow_objects",
            )
            .await
            .expect("legacy fixture may temporarily remove marker enforcement");
        exec(
            &state,
            "UPDATE flow_objects SET governance_metadata = governance_metadata - 'system_role' WHERE id = $1",
            vec![parent_id.into()],
        )
        .await;
        let migration = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../migrations/0059_flow_navigator_root.sql"
        ))
        .expect("0059 migration is readable");
        state
            .db
            .execute_unprepared(&migration)
            .await
            .expect("0059 replay adopts and marks the legacy project root");

        index_accepted_projection(&state, page_id).await;
        let mut current = search_query("Project top-level page");
        current.project_id = Some(project_id);
        current.all_visible = false;
        current.freshness = Some("require_current".to_string());
        let current = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(current),
            )
            .await,
        ))
        .await;
        assert_eq!(current["code"], 0, "adopted root poisoned require_current: {current}");
        assert_eq!(current["data"]["index_frontier"]["stale"], false, "{current}");
        assert_eq!(current["data"]["items"][0]["object"]["id"], page_id.to_string());

        scratch.drop_self().await;
    }

    // ---- `GET|PUT /workspaces/{workspace_id}/features/flow` ----

    #[tokio::test]
    async fn get_feature_on_a_never_provisioned_workspace_returns_the_column_defaults() {
        let scratch = scratch_or_skip!("feature-default");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_bare_workspace(&state).await;
        let claims = claims_for(owner_id);

        let response = to_response(get_flow_feature(State(state.clone()), claims, None, Path(workspace_id)).await);

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["flow_enabled"], false, "{body}");
        assert_eq!(body["data"]["default_member_level"], "edit", "{body}");
        assert_eq!(body["data"]["authz_epoch"], 0, "{body}");
        assert!(body["data"]["updated_at"].is_null(), "{body}");
        assert!(body["data"]["updated_by"].is_null(), "{body}");

        // A `GET` must be side-effect free: no row was provisioned by reading it.
        let count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_workspace_settings WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("count query runs")
            .expect("count query returns a row");
        let n: i64 = count.try_get("", "n").expect("count column reads");
        assert_eq!(n, 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn admin_put_enables_flow_and_the_change_is_persisted_and_visible_to_a_later_get() {
        let scratch = scratch_or_skip!("feature-put-persist");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_bare_workspace(&state).await;
        let claims = claims_for(owner_id);

        let put_response = to_response(
            set_flow_feature(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: Some(true),
                    default_member_level: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        );
        assert_eq!(put_response.status(), axum::http::StatusCode::OK);
        let put_body = body_json(put_response).await;
        assert_eq!(put_body["code"], 0, "{put_body}");
        assert_eq!(put_body["data"]["flow_enabled"], true, "{put_body}");
        assert!(!put_body["data"]["event_id"].is_null(), "{put_body}");
        assert!(!put_body["data"]["updated_at"].is_null(), "{put_body}");
        assert_eq!(put_body["data"]["updated_by"], owner_id.to_string(), "{put_body}");

        // A fresh `GET` — not the `PUT` handler's own return value — proves the write actually
        // reached the database rather than only being reflected in the response the handler built.
        let get_response = to_response(get_flow_feature(State(state.clone()), claims, None, Path(workspace_id)).await);
        let get_body = body_json(get_response).await;
        assert_eq!(get_body["data"]["flow_enabled"], true, "{get_body}");

        // Exactly one `flow.feature.enabled` business event, with a same-transaction `event_dispatch` row.
        let event_row = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, event_type FROM business_events WHERE workspace_id = $1 AND aggregate_type = 'flow_feature'",
                vec![workspace_id.into()],
            ))
            .await
            .expect("event query runs")
            .expect("exactly one flow_feature business event exists");
        let event_type: String = event_row.try_get("", "event_type").expect("event_type reads");
        assert_eq!(event_type, "flow.feature.enabled");
        let event_id: Uuid = event_row.try_get("", "id").expect("id reads");

        let dispatch_count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
                vec![event_id.into()],
            ))
            .await
            .expect("dispatch count query runs")
            .expect("dispatch count query returns a row");
        let n: i64 = dispatch_count.try_get("", "n").expect("count column reads");
        assert_eq!(n, 1, "exactly one event_dispatch row per business event");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn disabling_flow_advances_epoch_removes_presence_and_closes_workspace_sessions() {
        let scratch = scratch_or_skip!("feature-disable-sessions");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "feature disable session").await;
        let document_id = document_of(&state, object_id).await;
        let session_id = Uuid::new_v4();
        let registry = &crate::flow::collab::runtime::runtime().registry;
        let mut registered = registry
            .try_register_authorized(document_id, object_id, owner_id, workspace_id, session_id, 0)
            .expect("owner session registers");
        registry
            .upsert_presence(
                document_id,
                session_id,
                json!({"cursor": "owner"}),
                Duration::from_secs(30),
            )
            .expect("presence registers");

        let response = to_response(
            set_flow_feature(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: Some(false),
                    default_member_level: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        );
        let body = body_json(response).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["flow_enabled"], false, "{body}");
        assert_eq!(
            body["data"]["authz_epoch"], 1,
            "flag closure must advance the write fence"
        );
        let OutboundEvent::Close { code, reason } = registered.receiver.try_recv().expect("feature close is queued")
        else {
            panic!("expected a feature-disabled close")
        };
        assert_eq!(code, 4404);
        assert_eq!(reason, "feature disabled");
        assert_eq!(registry.presence_count(document_id), 0);
        assert_eq!(registry.session_count(document_id), 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_non_admin_member_cannot_put_the_feature_flag_via_body_code_not_http_status() {
        let scratch = scratch_or_skip!("feature-put-forbidden");
        let state = state_for(scratch.db.clone());
        let (workspace_id, _owner_id) = seed_bare_workspace(&state).await;
        let member_id = seed_member(&state, workspace_id).await;
        let claims = claims_for(member_id);

        let response = to_response(
            set_flow_feature(
                State(state.clone()),
                claims,
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: Some(true),
                    default_member_level: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        );

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 403, "{body}");
        assert!(body["data"].is_null(), "{body}");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn baseline_change_advances_epoch_emits_event_and_physically_invalidates_cache() {
        let scratch = scratch_or_skip!("feature-put-baseline");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_bare_workspace(&state).await;
        let claims = claims_for(owner_id);
        let poisoned_object_id = Uuid::new_v4();
        let cache = PermissionCache::for_state(&state).expect("permission cache is available");
        cache.put_for_test(
            workspace_id,
            PrincipalKind::User,
            owner_id,
            poisoned_object_id,
            PermissionLevel::FullAccess,
            1,
        );

        let response = to_response(
            set_flow_feature(
                State(state.clone()),
                claims,
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: None,
                    default_member_level: Some("full_access".to_string()),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        );

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["default_member_level"], "full_access", "{body}");
        assert_eq!(body["data"]["authz_epoch"], 1, "{body}");
        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, owner_id, poisoned_object_id, 1),
            None,
            "workspace cleanup must remove even a future-epoch poisoned entry"
        );

        let event = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, event_type, payload FROM business_events WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("event query runs")
            .expect("baseline event exists");
        let event_id: Uuid = event.try_get("", "id").expect("event id reads");
        let event_type: String = event.try_get("", "event_type").expect("event type reads");
        let payload: Value = event.try_get("", "payload").expect("event payload reads");
        assert_eq!(event_type, "flow.permission.baseline_changed");
        assert_eq!(payload["workspace_id"], workspace_id.to_string());
        assert_eq!(payload["old_level"], "edit");
        assert_eq!(payload["new_level"], "full_access");
        assert_eq!(body["data"]["event_id"], event_id.to_string());

        let dispatch = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
                vec![event_id.into()],
            ))
            .await
            .expect("dispatch query runs")
            .expect("dispatch count exists");
        assert_eq!(dispatch.try_get::<i64>("", "n").expect("count reads"), 1);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn member_add_role_change_and_remove_advance_epoch_in_their_transactions() {
        #[derive(FromQueryResult, PartialEq, Eq, Debug)]
        struct SettingsAuditStamp {
            updated_at: chrono::DateTime<chrono::Utc>,
            updated_by: Option<Uuid>,
        }

        let scratch = scratch_or_skip!("member-epoch-lifecycle");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let target_id = seed_user(&state).await;
        let settings_stamp_before = SettingsAuditStamp::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT updated_at, updated_by FROM flow_workspace_settings WHERE workspace_id = $1",
            vec![workspace_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("settings audit stamp query runs")
        .expect("seeded settings exist");
        let poisoned_object_id = Uuid::new_v4();
        let cache = PermissionCache::for_state(&state).expect("permission cache is available");
        cache.put_for_test(
            workspace_id,
            PrincipalKind::User,
            target_id,
            poisoned_object_id,
            PermissionLevel::FullAccess,
            1,
        );

        let rejected = body_json(to_response(
            remove_member(
                State(state.clone()),
                claims_for(owner_id),
                Path((workspace_id, owner_id)),
            )
            .await,
        ))
        .await;
        assert_eq!(rejected["code"], 403, "{rejected}");
        assert_eq!(
            read_epoch(&state, workspace_id).await,
            0,
            "a rejected member mutation must roll its epoch advance back"
        );

        tokio::time::sleep(Duration::from_millis(5)).await;

        let added = body_json(to_response(
            add_member(
                State(state.clone()),
                claims_for(owner_id),
                Path(workspace_id),
                Json(AddMemberRequest {
                    user_id: target_id,
                    role: "member".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(added["code"], 0, "{added}");
        assert_eq!(added["data"]["role"], "member", "response shape changed: {added}");
        assert_eq!(read_epoch(&state, workspace_id).await, 1);
        let settings_stamp_after = SettingsAuditStamp::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT updated_at, updated_by FROM flow_workspace_settings WHERE workspace_id = $1",
            vec![workspace_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("settings audit stamp query runs")
        .expect("settings still exist");
        assert_eq!(
            settings_stamp_after, settings_stamp_before,
            "membership-only epoch advancement must not rewrite Flow settings audit metadata"
        );
        assert_eq!(
            cache.get(workspace_id, PrincipalKind::User, target_id, poisoned_object_id, 1),
            None,
            "member mutation must physically invalidate the workspace cache"
        );

        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "member revoke session").await;
        let document_id = document_of(&state, object_id).await;
        let session_id = Uuid::new_v4();
        let registry = &crate::flow::collab::runtime::runtime().registry;
        let mut registered = registry
            .try_register_authorized(document_id, object_id, target_id, workspace_id, session_id, 1)
            .expect("member session registers at epoch one");
        registry
            .upsert_presence(
                document_id,
                session_id,
                json!({"cursor": "member"}),
                Duration::from_secs(30),
            )
            .expect("member presence registers");

        let updated = body_json(to_response(
            update_member_role(
                State(state.clone()),
                claims_for(owner_id),
                Path((workspace_id, target_id)),
                Json(UpdateMemberRoleRequest {
                    role: "admin".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(updated["code"], 0, "{updated}");
        assert_eq!(read_epoch(&state, workspace_id).await, 2);
        assert!(
            registered.receiver.try_recv().is_err(),
            "promotion keeps the still-authorized session connected"
        );
        assert_eq!(registry.presence_count(document_id), 1);

        let removed = body_json(to_response(
            remove_member(
                State(state.clone()),
                claims_for(owner_id),
                Path((workspace_id, target_id)),
            )
            .await,
        ))
        .await;
        assert_eq!(removed["code"], 0, "{removed}");
        assert_eq!(read_epoch(&state, workspace_id).await, 3);
        let OutboundEvent::Close { code, reason } = registered.receiver.try_recv().expect("removal closes session")
        else {
            panic!("expected an authorization close")
        };
        assert_eq!(code, 4403);
        assert_eq!(reason, "authorization revoked");
        assert_eq!(
            registry.presence_count(document_id),
            0,
            "member removal deletes presence immediately"
        );
        assert_eq!(registry.session_count(document_id), 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn external_guest_sees_only_objects_reached_by_an_explicit_flow_grant() {
        let scratch = scratch_or_skip!("external-guest-grant");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let guest_id = seed_user(&state).await;
        let visible_id = create_page_as_owner(&state, workspace_id, owner_id, "guest-visible").await;
        let hidden_id = create_page_as_owner(&state, workspace_id, owner_id, "guest-hidden").await;

        let before = body_json(to_response(
            list_flow_objects(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: None,
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(before["code"], 0, "{before}");
        assert!(before["data"]["items"].as_array().expect("items").is_empty());

        let granted = body_json(to_response(
            put_flow_object_grants(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(visible_id),
                Json(SetGrantsRequest {
                    grants: vec![GrantRequestBody {
                        principal_kind: "user".to_string(),
                        principal_id: guest_id,
                        level: "view".to_string(),
                    }],
                    confirm_self_lockout: true,
                    dry_run: false,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(granted["code"], 0, "{granted}");

        let after = body_json(to_response(
            list_flow_objects(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Path(workspace_id),
                Query(ListFlowObjectsQuery {
                    project_id: None,
                    unprojected: false,
                    object_type: None,
                    parent_id: None,
                    q: None,
                    cursor: None,
                    limit: None,
                    include_archived: false,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(after["code"], 0, "{after}");
        let ids: Vec<String> = after["data"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|item| item["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(ids, vec![visible_id.to_string()]);
        assert!(!ids.contains(&hidden_id.to_string()));

        let visible = body_json(to_response(
            get_flow_object(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Path(visible_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(visible["code"], 0, "{visible}");

        let hidden = body_json(to_response(
            get_flow_object(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Path(hidden_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(hidden["code"], 404, "{hidden}");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_bridge_guest_routes_hide_reference_cardinality_and_target_existence() {
        use base64::Engine as _;

        #[derive(FromQueryResult)]
        struct Frontier {
            head_frontier: Vec<u8>,
        }

        let scratch = scratch_or_skip!("bridge-guest-non-enumeration");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let guest_id = seed_user(&state).await;
        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'BGN', 'Bridge guest non-enumeration', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;
        let form = body_json(to_response(
            create_project_form(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(project_id),
                Json(CreateFormRequest {
                    key: "guest_hidden_form".to_string(),
                    name: "Guest hidden form".to_string(),
                    description: None,
                    icon: None,
                    color: None,
                    title_template: None,
                    schema: None,
                    detail_layout: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(form["code"], 0, "{form}");
        let form_id = Uuid::parse_str(form["data"]["id"].as_str().expect("form id")).expect("UUID");
        let source_id = create_page_as_owner(&state, workspace_id, owner_id, "guest bridge source").await;

        let created = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(crate::flow::bridge::CreateReferenceInput {
                    target_type: "form".to_string(),
                    target_id: form_id,
                    display: json!({"mode":"embed"}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(created["code"], 0, "{created}");

        let set_guest_level = |level: &str| {
            put_flow_object_grants(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(SetGrantsRequest {
                    grants: vec![GrantRequestBody {
                        principal_kind: "user".to_string(),
                        principal_id: guest_id,
                        level: level.to_string(),
                    }],
                    confirm_self_lockout: true,
                    dry_run: false,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
        };
        assert_eq!(body_json(to_response(set_guest_level("view").await)).await["code"], 0);
        let view_only_create = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Path(source_id),
                Json(crate::flow::bridge::CreateReferenceInput {
                    target_type: "form".to_string(),
                    target_id: form_id,
                    display: json!({}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(view_only_create["code"], 404, "{view_only_create}");

        assert_eq!(body_json(to_response(set_guest_level("edit").await)).await["code"], 0);
        let guest_list = body_json(to_response(
            get_flow_object_references(State(state.clone()), claims_for(guest_id), None, Path(source_id)).await,
        ))
        .await;
        assert_eq!(guest_list["code"], 0, "{guest_list}");
        assert_eq!(guest_list["data"]["items"], json!([]), "{guest_list}");

        let create_as_guest = |target_id| {
            post_flow_object_reference(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Path(source_id),
                Json(crate::flow::bridge::CreateReferenceInput {
                    target_type: "form".to_string(),
                    target_id,
                    display: json!({}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
        };
        let existing_reference = body_json(to_response(create_as_guest(form_id).await)).await;
        let missing_reference = body_json(to_response(create_as_guest(Uuid::new_v4()).await)).await;
        assert_eq!(existing_reference, missing_reference);
        assert_eq!(existing_reference["code"], 404, "{existing_reference}");
        assert!(existing_reference.get("details").is_none());

        let frontier = Frontier::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("frontier query")
        .expect("frontier");
        let frontier = base64::engine::general_purpose::STANDARD.encode(frontier.head_frontier);
        let preview_as_guest = |target_type: &str, mapping: Value| {
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(guest_id),
                None,
                Json(crate::flow::bridge::ConversionPreviewInput {
                    source_object_id: source_id,
                    source_frontier: frontier.clone(),
                    target_type: target_type.to_string(),
                    mapping,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
        };
        let existing_preview = body_json(to_response(
            preview_as_guest("form_record", json!({"target_form_id":form_id,"values":{}})).await,
        ))
        .await;
        let missing_preview = body_json(to_response(
            preview_as_guest("form_record", json!({"target_form_id":Uuid::new_v4(),"values":{}})).await,
        ))
        .await;
        assert_eq!(existing_preview, missing_preview);
        assert_eq!(existing_preview["code"], 404, "{existing_preview}");
        let create_form_preview = body_json(to_response(
            preview_as_guest("form", json!({"target_project_id":project_id,"schema":{"fields":[]}})).await,
        ))
        .await;
        assert_eq!(create_form_preview["code"], 404, "{create_form_preview}");

        let archived = body_json(to_response(
            delete_form(State(state.clone()), claims_for(owner_id), None, Path(form_id)).await,
        ))
        .await;
        assert_eq!(archived["code"], 0, "{archived}");
        let unavailable = body_json(to_response(
            get_flow_object_references(State(state.clone()), claims_for(owner_id), None, Path(source_id)).await,
        ))
        .await;
        assert_eq!(unavailable["data"]["items"], json!([{"visibility":"unavailable"}]));

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_bridge_reference_embed_reauthorizes_forms_policy_and_missing_policy_is_read_only() {
        #[derive(FromQueryResult, PartialEq, Eq, Debug)]
        struct SourceHead {
            head_seq: i64,
            head_frontier: Vec<u8>,
        }

        let scratch = scratch_or_skip!("bridge-reference-reauth");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let baseline = body_json(to_response(
            set_flow_feature(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: Some(true),
                    default_member_level: Some("view".to_string()),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(baseline["code"], 0, "{baseline}");
        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'BRG', 'Bridge test', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;
        let form = body_json(to_response(
            create_project_form(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(project_id),
                Json(CreateFormRequest {
                    key: "bridge_form".to_string(),
                    name: "Sensitive form".to_string(),
                    description: Some("must not be cached".to_string()),
                    icon: None,
                    color: None,
                    title_template: None,
                    schema: Some(json!({
                        "version":"openpr.form.schema.v1",
                        "fields":[
                            {"field_id":"fld_public","key":"public","type":"text"},
                            {"field_id":"fld_private","key":"private","type":"text"}
                        ]
                    })),
                    detail_layout: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(form["code"], 0, "{form}");
        let form_id = Uuid::parse_str(form["data"]["id"].as_str().expect("form id")).expect("UUID");
        let source_id = create_page_as_owner(&state, workspace_id, owner_id, "bridge source").await;

        let member_policy = body_json(to_response(
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "member".to_string(),
                        policy: json!({"actions":{"form.view":true}}),
                    }],
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(member_policy["code"], 0, "{member_policy}");
        let view_grant = body_json(to_response(
            put_flow_object_grants(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(SetGrantsRequest {
                    grants: vec![GrantRequestBody {
                        principal_kind: "user".to_string(),
                        principal_id: member_id,
                        level: "view".to_string(),
                    }],
                    confirm_self_lockout: true,
                    dry_run: false,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(view_grant["code"], 0, "{view_grant}");
        let view_only_create = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(source_id),
                Json(crate::flow::bridge::CreateReferenceInput {
                    target_type: "form".to_string(),
                    target_id: form_id,
                    display: json!({}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(view_only_create["code"], 404, "{view_only_create}");
        let no_member_write = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_bridge_references WHERE source_object_id=$1",
                vec![source_id.into()],
            ))
            .await
            .expect("member reference count query")
            .expect("member reference count");
        assert_eq!(no_member_write.try_get::<i64>("", "n").expect("count"), 0);
        let source_head = SourceHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq, head_frontier FROM collab_documents WHERE object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("source head query")
        .expect("source head");

        let reference_key = Uuid::new_v4().to_string();
        let create_input = || crate::flow::bridge::CreateReferenceInput {
            target_type: "form".to_string(),
            target_id: form_id,
            display: json!({"mode": "embed"}),
            idempotency_key: reference_key.clone(),
        };
        let created = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(create_input()),
            )
            .await,
        ))
        .await;
        assert_eq!(created["code"], 0, "{created}");
        assert_eq!(created["data"]["permission_state"]["access"], "read_only");
        assert_eq!(created["data"]["permission_state"]["configuration"], "unconfigured");
        assert_eq!(created["data"]["permission_state"]["actions"], json!(["form.view"]));
        let reference_id =
            Uuid::parse_str(created["data"]["reference_id"].as_str().expect("reference id")).expect("UUID");

        let replay = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(create_input()),
            )
            .await,
        ))
        .await;
        assert_eq!(replay["code"], 0, "{replay}");
        assert_eq!(replay["data"]["reference_id"], reference_id.to_string());

        let before = body_json(to_response(
            get_flow_object_references(State(state.clone()), claims_for(owner_id), None, Path(source_id)).await,
        ))
        .await;
        assert_eq!(before["code"], 0, "{before}");
        assert_eq!(before["data"]["items"][0]["visibility"], "available");
        assert_eq!(before["data"]["items"][0]["title"], "Sensitive form");

        let record = body_json(to_response(
            create_form_record(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(CreateRecordRequest {
                    values: json!({"public":"visible","private":"redact-me"}),
                    title: Some("Sensitive record".to_string()),
                    source: None,
                    idempotency_key: Some(Uuid::new_v4().to_string()),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(record["code"], 0, "{record}");
        let record_id = Uuid::parse_str(record["data"]["id"].as_str().expect("record id")).expect("UUID");
        let record_reference = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(crate::flow::bridge::CreateReferenceInput {
                    target_type: "form_record".to_string(),
                    target_id: record_id,
                    display: json!({}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(record_reference["code"], 0, "{record_reference}");

        let restricted = body_json(to_response(
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "owner".to_string(),
                        policy: json!({
                            "actions":{"form.view":true,"record.create":true},
                            "fields":{"private":{"read":false,"write":false}}
                        }),
                    }],
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(restricted["code"], 0, "{restricted}");
        let redacted = body_json(to_response(
            get_flow_object_references(State(state.clone()), claims_for(owner_id), None, Path(source_id)).await,
        ))
        .await;
        assert_eq!(redacted["code"], 0, "{redacted}");
        let record_item = redacted["data"]["items"]
            .as_array()
            .expect("reference items")
            .iter()
            .find(|item| item["target_id"] == record_id.to_string())
            .expect("record reference");
        assert_eq!(record_item["summary"], json!({"public":"visible"}));
        assert_eq!(record_item["permission_state"]["field_read_limited"], true);

        exec(
            &state,
            "UPDATE flow_workspace_settings SET bridge_enabled=false WHERE workspace_id=$1",
            vec![workspace_id.into()],
        )
        .await;
        let disabled_list = body_json(to_response(
            get_flow_object_references(State(state.clone()), claims_for(owner_id), None, Path(source_id)).await,
        ))
        .await;
        assert_eq!(disabled_list["code"], 0, "{disabled_list}");
        assert!(
            disabled_list["data"]["items"]
                .as_array()
                .expect("disabled items")
                .iter()
                .all(|item| item["permission_state"]["access"] == "read_only"
                    && item["permission_state"]["actions"] == json!(["form.view"])),
            "disabled bridge must downgrade every visible reference: {disabled_list}"
        );
        exec(
            &state,
            "UPDATE flow_workspace_settings SET bridge_enabled=true WHERE workspace_id=$1",
            vec![workspace_id.into()],
        )
        .await;

        let policy = body_json(to_response(
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "owner".to_string(),
                        policy: json!({"actions": {"form.view": false}}),
                    }],
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(policy["code"], 0, "{policy}");

        let after = body_json(to_response(
            get_flow_object_references(State(state.clone()), claims_for(owner_id), None, Path(source_id)).await,
        ))
        .await;
        assert_eq!(after["code"], 0, "{after}");
        assert_eq!(after["data"]["items"], json!([]));

        let remove_key = Uuid::new_v4().to_string();
        let removed = body_json(to_response(
            delete_flow_object_reference(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path((source_id, reference_id)),
                HeaderMap::from_iter([(
                    axum::http::header::HeaderName::from_static("idempotency-key"),
                    axum::http::HeaderValue::from_str(&remove_key).expect("header"),
                )]),
            )
            .await,
        ))
        .await;
        assert_eq!(removed["code"], 0, "{removed}");
        assert_eq!(removed["data"]["removed"], true);
        let after_reference_commands = SourceHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq, head_frontier FROM collab_documents WHERE object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("source head query")
        .expect("source head");
        assert_eq!(
            after_reference_commands, source_head,
            "reference and unreference must not advance an existing document head"
        );

        let target_count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM project_forms WHERE id = $1",
                vec![form_id.into()],
            ))
            .await
            .expect("target count query")
            .expect("target count");
        assert_eq!(target_count.try_get::<i64>("", "n").expect("count"), 1);

        exec(
            &state,
            "UPDATE flow_workspace_settings SET bridge_enabled=false WHERE workspace_id=$1",
            vec![workspace_id.into()],
        )
        .await;
        let disabled = body_json(to_response(
            post_flow_object_reference(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(crate::flow::bridge::CreateReferenceInput {
                    target_type: "form".to_string(),
                    target_id: form_id,
                    display: json!({}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(disabled["code"], 403, "{disabled}");

        scratch.drop_self().await;
    }

    #[tokio::test]
    #[allow(clippy::items_after_statements)]
    async fn flow_bridge_conversion_commit_rechecks_policy_and_is_idempotent_without_rewriting_source() {
        use base64::Engine as _;

        #[derive(FromQueryResult)]
        struct SourceHead {
            head_seq: i64,
            head_frontier: Vec<u8>,
        }

        let scratch = scratch_or_skip!("bridge-conversion-policy");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'CNV', 'Conversion test', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;
        let form = body_json(to_response(
            create_project_form(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(project_id),
                Json(CreateFormRequest {
                    key: "conversion_target".to_string(),
                    name: "Conversion target".to_string(),
                    description: None,
                    icon: None,
                    color: None,
                    title_template: None,
                    schema: None,
                    detail_layout: None,
                }),
            )
            .await,
        ))
        .await;
        let form_id = Uuid::parse_str(form["data"]["id"].as_str().expect("form id")).expect("UUID");
        let set_policy = |allowed: bool| {
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "owner".to_string(),
                        policy: json!({"actions":{"form.view":true,"record.create":allowed}}),
                    }],
                }),
            )
        };
        assert_eq!(body_json(to_response(set_policy(true).await)).await["code"], 0);

        let source_id = create_page_as_owner(&state, workspace_id, owner_id, "conversion source").await;
        let source_head = SourceHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT d.head_seq, d.head_frontier FROM collab_documents d WHERE d.object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("source head query")
        .expect("source head");
        let frontier = base64::engine::general_purpose::STANDARD.encode(&source_head.head_frontier);
        let preview_request = |key: String| crate::flow::bridge::ConversionPreviewInput {
            source_object_id: source_id,
            source_frontier: frontier.clone(),
            target_type: "form_record".to_string(),
            mapping: json!({"target_form_id":form_id,"title":"Converted","values":{}}),
            idempotency_key: key,
        };
        let preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(preview_request(Uuid::new_v4().to_string())),
            )
            .await,
        ))
        .await;
        assert_eq!(preview["code"], 0, "{preview}");
        let preview_id = Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");

        assert_eq!(body_json(to_response(set_policy(false).await)).await["code"], 0);
        let rejected = body_json(to_response(
            post_flow_conversion(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionCommitInput {
                    preview_id,
                    source_frontier: frontier.clone(),
                    target_schema_version: 1,
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(rejected["code"], 403, "{rejected}");
        let rejected_actions = rejected["details"]["permission_state"]["actions"]
            .as_array()
            .expect("permission actions");
        assert!(
            rejected_actions.contains(&json!("form.view")) && !rejected_actions.contains(&json!("record.create")),
            "commit rejection must return the current decision with record.create removed: {rejected}"
        );

        let zero = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT (SELECT count(*) FROM form_records WHERE form_id=$1) AS records, \
                    (SELECT count(*) FROM flow_object_lineage WHERE source_object_id=$2) AS lineage",
                vec![form_id.into(), source_id.into()],
            ))
            .await
            .expect("zero-write query")
            .expect("counts");
        assert_eq!(zero.try_get::<i64>("", "records").expect("records"), 0);
        assert_eq!(zero.try_get::<i64>("", "lineage").expect("lineage"), 0);

        assert_eq!(body_json(to_response(set_policy(true).await)).await["code"], 0);
        let preview_key = Uuid::new_v4().to_string();
        let preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(preview_request(preview_key.clone())),
            )
            .await,
        ))
        .await;
        let preview_id = Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");
        let preview_replay = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(preview_request(preview_key.clone())),
            )
            .await,
        ))
        .await;
        assert_eq!(preview_replay["data"]["preview_id"], preview_id.to_string());
        let preview_collision = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionPreviewInput {
                    source_object_id: source_id,
                    source_frontier: frontier.clone(),
                    target_type: "form_record".to_string(),
                    mapping: json!({"target_form_id":form_id,"title":"different","values":{}}),
                    idempotency_key: preview_key,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(preview_collision["code"], 400, "{preview_collision}");
        let commit_key = Uuid::new_v4().to_string();
        let commit_input = || crate::flow::bridge::ConversionCommitInput {
            preview_id,
            source_frontier: frontier.clone(),
            target_schema_version: 1,
            idempotency_key: commit_key.clone(),
            confirm: true,
        };
        let committed_result =
            post_flow_conversion(State(state.clone()), claims_for(owner_id), None, Json(commit_input())).await;
        if let Err(error) = &committed_result {
            panic!("conversion commit failed: {error:?}");
        }
        let committed = body_json(to_response(committed_result)).await;
        assert_eq!(committed["code"], 0, "{committed}");
        assert_eq!(committed["data"]["status"], "completed");
        assert_eq!(
            committed["data"]["created_target_ids"].as_array().map(Vec::len),
            Some(1)
        );
        let job_id = Uuid::parse_str(committed["data"]["job_id"].as_str().expect("job id")).expect("UUID");
        let status = body_json(to_response(
            get_flow_conversion(State(state.clone()), claims_for(owner_id), None, Path(job_id)).await,
        ))
        .await;
        assert_eq!(status["code"], 0, "{status}");
        assert_eq!(status["data"]["status"], "completed");
        let replay = body_json(to_response(
            post_flow_conversion(State(state.clone()), claims_for(owner_id), None, Json(commit_input())).await,
        ))
        .await;
        assert_eq!(replay["data"]["job_id"], committed["data"]["job_id"]);
        assert_eq!(
            replay["data"]["created_target_ids"],
            committed["data"]["created_target_ids"]
        );

        let after_head = SourceHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT d.head_seq, d.head_frontier FROM collab_documents d WHERE d.object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("source head query")
        .expect("source head");
        assert_eq!(after_head.head_seq, source_head.head_seq);
        assert_eq!(after_head.head_frontier, source_head.head_frontier);

        struct FaultReset;
        impl Drop for FaultReset {
            fn drop(&mut self) {
                crate::flow::bridge::set_conversion_fault_for_test(None, 0);
            }
        }
        let _fault_reset = FaultReset;
        for fault in [1_u8, 2_u8] {
            let preview = body_json(to_response(
                post_flow_conversion_preview(
                    State(state.clone()),
                    claims_for(owner_id),
                    None,
                    Json(preview_request(Uuid::new_v4().to_string())),
                )
                .await,
            ))
            .await;
            let fault_preview_id =
                Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");
            crate::flow::bridge::set_conversion_fault_for_test(Some(fault_preview_id), fault);
            let failed = body_json(to_response(
                post_flow_conversion(
                    State(state.clone()),
                    claims_for(owner_id),
                    None,
                    Json(crate::flow::bridge::ConversionCommitInput {
                        preview_id: fault_preview_id,
                        source_frontier: frontier.clone(),
                        target_schema_version: 1,
                        idempotency_key: Uuid::new_v4().to_string(),
                        confirm: true,
                    }),
                )
                .await,
            ))
            .await;
            crate::flow::bridge::set_conversion_fault_for_test(None, 0);
            assert_eq!(failed["code"], 0, "fault {fault}: {failed}");
            assert_eq!(failed["data"]["status"], "failed", "fault {fault}: {failed}");
            let failed_job_id = Uuid::parse_str(failed["data"]["job_id"].as_str().expect("job id")).expect("UUID");
            let retried = body_json(to_response(
                post_flow_conversion_retry(
                    State(state.clone()),
                    claims_for(owner_id),
                    None,
                    Path(failed_job_id),
                    Json(crate::flow::bridge::ConversionRetryInput {
                        idempotency_key: Uuid::new_v4().to_string(),
                        confirm: true,
                    }),
                )
                .await,
            ))
            .await;
            assert_eq!(retried["code"], 0, "fault {fault} retry: {retried}");
            assert_eq!(retried["data"]["status"], "completed");
            assert_eq!(retried["data"]["created_target_ids"].as_array().map(Vec::len), Some(1));
        }

        let expiring_preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(preview_request(Uuid::new_v4().to_string())),
            )
            .await,
        ))
        .await;
        let expiring_preview_id =
            Uuid::parse_str(expiring_preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");
        crate::flow::bridge::set_conversion_fault_for_test(Some(expiring_preview_id), 1);
        let expired_failed = body_json(to_response(
            post_flow_conversion(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionCommitInput {
                    preview_id: expiring_preview_id,
                    source_frontier: frontier.clone(),
                    target_schema_version: 1,
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        crate::flow::bridge::set_conversion_fault_for_test(None, 0);
        let expired_job_id =
            Uuid::parse_str(expired_failed["data"]["job_id"].as_str().expect("failed job id")).expect("UUID");
        exec(
            &state,
            "UPDATE flow_conversion_previews SET expires_at=now()-interval '1 second' WHERE id=$1",
            vec![expiring_preview_id.into()],
        )
        .await;
        let expired_retry = body_json(to_response(
            post_flow_conversion_retry(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(expired_job_id),
                Json(crate::flow::bridge::ConversionRetryInput {
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(expired_retry["code"], 403, "{expired_retry}");

        let preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(preview_request(Uuid::new_v4().to_string())),
            )
            .await,
        ))
        .await;
        let last_preview_id =
            Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");
        let last_key = Uuid::new_v4().to_string();
        let last_input = || crate::flow::bridge::ConversionCommitInput {
            preview_id: last_preview_id,
            source_frontier: frontier.clone(),
            target_schema_version: 1,
            idempotency_key: last_key.clone(),
            confirm: true,
        };
        crate::flow::bridge::set_conversion_fault_for_test(Some(last_preview_id), 3);
        let lost_response =
            post_flow_conversion(State(state.clone()), claims_for(owner_id), None, Json(last_input())).await;
        crate::flow::bridge::set_conversion_fault_for_test(None, 0);
        assert!(
            lost_response.is_err(),
            "post-commit response injection must surface an error"
        );
        let recovered = body_json(to_response(
            post_flow_conversion(State(state.clone()), claims_for(owner_id), None, Json(last_input())).await,
        ))
        .await;
        assert_eq!(recovered["code"], 0, "{recovered}");
        assert_eq!(recovered["data"]["status"], "completed");

        let totals = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT (SELECT count(*) FROM form_records WHERE form_id=$1) AS records, \
                        (SELECT count(*) FROM flow_object_lineage WHERE source_object_id=$2) AS lineage",
                vec![form_id.into(), source_id.into()],
            ))
            .await
            .expect("conversion totals query")
            .expect("conversion totals");
        assert_eq!(totals.try_get::<i64>("", "records").expect("records"), 4);
        assert_eq!(totals.try_get::<i64>("", "lineage").expect("lineage"), 4);

        let events = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \
                   (SELECT count(*) FROM form_events WHERE form_id=$1 AND event_type='form.record.created' \
                    AND source->>'type'='flow_conversion') AS native_events, \
                   (SELECT count(*) FROM business_events WHERE project_id=$2 AND event_type='form.record.created' \
                    AND source->>'type'='flow_conversion') AS native_business_events, \
                   (SELECT count(*) FROM business_events WHERE workspace_id=$3 \
                    AND event_type='flow.conversion.completed') AS completed_events",
                vec![form_id.into(), project_id.into(), workspace_id.into()],
            ))
            .await
            .expect("event totals query")
            .expect("event totals");
        assert_eq!(events.try_get::<i64>("", "native_events").expect("native events"), 4);
        assert_eq!(
            events
                .try_get::<i64>("", "native_business_events")
                .expect("native business events"),
            4
        );
        assert_eq!(
            events.try_get::<i64>("", "completed_events").expect("completed events"),
            4
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_bridge_record_conversion_runs_native_autonumber_and_validator_pipeline() {
        use base64::Engine as _;

        #[derive(FromQueryResult)]
        struct Frontier {
            head_frontier: Vec<u8>,
        }

        let scratch = scratch_or_skip!("bridge-native-record-pipeline");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'BNP', 'Bridge native pipeline', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;
        let form = body_json(to_response(
            create_project_form(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(project_id),
                Json(CreateFormRequest {
                    key: "native_pipeline".to_string(),
                    name: "Native pipeline".to_string(),
                    description: None,
                    icon: None,
                    color: None,
                    title_template: Some("{ticket_no}".to_string()),
                    schema: Some(json!({
                        "version":"openpr.form.schema.v1",
                        "fields":[
                            {
                                "field_id":"fld_ticket_no",
                                "key":"ticket_no",
                                "type":"autonumber",
                                "required":true,
                                "autonumber":{"prefix":"BR-","width":4}
                            },
                            {
                                "field_id":"fld_checked",
                                "key":"checked",
                                "type":"text",
                                "required":true
                            }
                        ]
                    })),
                    detail_layout: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(form["code"], 0, "{form}");
        let form_id = Uuid::parse_str(form["data"]["id"].as_str().expect("form id")).expect("UUID");
        let policy = body_json(to_response(
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "owner".to_string(),
                        policy: json!({
                            "actions":{"form.view":true,"record.create":true},
                            "fields":{"checked":{"read":true,"write":false}}
                        }),
                    }],
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(policy["code"], 0, "{policy}");

        exec(
            &state,
            "INSERT INTO plugins \
             (id, workspace_id, project_id, key, name, version, manifest, wasm_bytes, status, installed_by) \
             VALUES ($1,$2,$3,'bridge_validator','Bridge validator','1.0.0',$4,$5,'active',$6)",
            vec![
                Uuid::new_v4().into(),
                workspace_id.into(),
                project_id.into(),
                json!({
                    "schema_version":"openpr.plugin.v1",
                    "key":"bridge_validator",
                    "name":"Bridge validator",
                    "version":"1.0.0",
                    "capabilities":{
                        "hooks":[{
                            "kind":"field_validator",
                            "form_key":"native_pipeline",
                            "field_key":"checked"
                        }],
                        "runtime":{"timeout_ms":500,"fuel":100_000,"memory_bytes":1_048_576}
                    }
                })
                .into(),
                validator_ok_wasm().into(),
                owner_id.into(),
            ],
        )
        .await;

        let source_id = create_page_as_owner(&state, workspace_id, owner_id, "native pipeline source").await;
        let head = Frontier::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("frontier query")
        .expect("frontier");
        let frontier = base64::engine::general_purpose::STANDARD.encode(head.head_frontier);
        let denied_preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionPreviewInput {
                    source_object_id: source_id,
                    source_frontier: frontier.clone(),
                    target_type: "form_record".to_string(),
                    mapping: json!({"target_form_id":form_id,"values":{"checked":"denied"}}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(denied_preview["code"], 0, "{denied_preview}");
        let denied_preview_id =
            Uuid::parse_str(denied_preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");
        let field_denied = body_json(to_response(
            post_flow_conversion(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionCommitInput {
                    preview_id: denied_preview_id,
                    source_frontier: frontier.clone(),
                    target_schema_version: 1,
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(field_denied["code"], 403, "{field_denied}");
        assert_eq!(
            field_denied["message"], "mapped values include a field that is not writable",
            "the bridge commit field-policy branch must run before native persistence: {field_denied}"
        );
        let allow_field = body_json(to_response(
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "owner".to_string(),
                        policy: json!({"actions":{"form.view":true,"record.create":true}}),
                    }],
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(allow_field["code"], 0, "{allow_field}");
        let preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionPreviewInput {
                    source_object_id: source_id,
                    source_frontier: frontier.clone(),
                    target_type: "form_record".to_string(),
                    mapping: json!({"target_form_id":form_id,"values":{"checked":"yes"}}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(preview["code"], 0, "{preview}");
        let preview_id = Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");
        let committed = body_json(to_response(
            post_flow_conversion(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Json(crate::flow::bridge::ConversionCommitInput {
                    preview_id,
                    source_frontier: frontier.clone(),
                    target_schema_version: 1,
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(committed["code"], 0, "{committed}");
        let target_id = Uuid::parse_str(
            committed["data"]["created_target_ids"][0]
                .as_str()
                .expect("created target id"),
        )
        .expect("UUID");
        let proof = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT r.values->>'ticket_no' AS ticket_no, r.title, \
                    (SELECT count(*) FROM plugin_invocations i WHERE i.project_id=$2 \
                     AND i.hook_kind='field_validator' AND i.status='completed') AS validator_calls \
                 FROM form_records r WHERE r.id=$1",
                vec![target_id.into(), project_id.into()],
            ))
            .await
            .expect("native pipeline proof query")
            .expect("converted record");
        assert_eq!(
            proof.try_get::<String>("", "ticket_no").expect("ticket number"),
            "BR-0001"
        );
        assert_eq!(
            proof.try_get::<String>("", "title").expect("title"),
            "native pipeline source"
        );
        assert_eq!(proof.try_get::<i64>("", "validator_calls").expect("validator calls"), 1);

        let trigger_probe = state
            .db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM pg_trigger t \
                 JOIN pg_class c ON c.oid=t.tgrelid \
                 WHERE NOT t.tgisinternal AND c.relname IN \
                 ('flow_bridge_references','flow_conversion_previews','flow_conversion_jobs',\
                  'flow_object_lineage','project_forms','form_records')",
            ))
            .await
            .expect("pg_trigger probe")
            .expect("trigger count");
        assert_eq!(
            trigger_probe.try_get::<i64>("", "n").expect("trigger count"),
            0,
            "bridge and target tables must have no user synchronization triggers"
        );
        let counts = |state: AppState| async move {
            state
                .db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT \
                       (SELECT count(*) FROM form_records WHERE id=$1) AS records, \
                       (SELECT count(*) FROM flow_object_lineage WHERE target_id=$1) AS lineage, \
                       (SELECT count(*) FROM form_events WHERE record_id=$1) AS native_events",
                    vec![target_id.into()],
                ))
                .await
                .expect("runtime count poll")
                .expect("runtime counts")
        };
        let before_poll = counts(state.clone()).await;
        let before_poll = (
            before_poll.try_get::<i64>("", "records").expect("records"),
            before_poll.try_get::<i64>("", "lineage").expect("lineage"),
            before_poll.try_get::<i64>("", "native_events").expect("events"),
        );
        crate::routes::form::process_pending_form_jobs_from_worker(state.clone(), 4)
            .await
            .expect("worker Forms tick");
        crate::routes::proposal::governance_tick(&state)
            .await
            .expect("worker governance tick");
        let worker_client = reqwest::Client::builder().build().expect("worker client");
        let _dispatch_report = crate::events::dispatcher::run_tick(&state, &worker_client, 4).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let after_poll = counts(state.clone()).await;
        let after_poll = (
            after_poll.try_get::<i64>("", "records").expect("records"),
            after_poll.try_get::<i64>("", "lineage").expect("lineage"),
            after_poll.try_get::<i64>("", "native_events").expect("events"),
        );
        assert_eq!(before_poll, (1, 1, 1));
        assert_eq!(
            after_poll, before_poll,
            "runtime poll observed a background double write"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_bridge_new_form_conversion_requires_admin_at_preview_and_commit() {
        use base64::Engine as _;

        #[derive(FromQueryResult)]
        struct Frontier {
            head_frontier: Vec<u8>,
        }

        let scratch = scratch_or_skip!("bridge-form-admin-recheck");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'BFA', 'Bridge form authority', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;
        let source_id = create_page_as_owner(&state, workspace_id, owner_id, "form authority source").await;
        let grant = body_json(to_response(
            put_flow_object_grants(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source_id),
                Json(SetGrantsRequest {
                    grants: vec![GrantRequestBody {
                        principal_kind: "user".to_string(),
                        principal_id: member_id,
                        level: "edit".to_string(),
                    }],
                    confirm_self_lockout: true,
                    dry_run: false,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(grant["code"], 0, "{grant}");
        let head = Frontier::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("frontier query")
        .expect("frontier");
        let frontier = base64::engine::general_purpose::STANDARD.encode(head.head_frontier);
        let preview_input = || crate::flow::bridge::ConversionPreviewInput {
            source_object_id: source_id,
            source_frontier: frontier.clone(),
            target_type: "form".to_string(),
            mapping: json!({
                "target_project_id":project_id,
                "key":format!("converted_{}",Uuid::new_v4().simple()),
                "name":"Converted form",
                "schema":{"version":"openpr.form.schema.v1","fields":[]}
            }),
            idempotency_key: Uuid::new_v4().to_string(),
        };
        let member_preview = body_json(to_response(
            post_flow_conversion_preview(State(state.clone()), claims_for(member_id), None, Json(preview_input()))
                .await,
        ))
        .await;
        assert_eq!(member_preview["code"], 403, "{member_preview}");

        let promoted = body_json(to_response(
            update_member_role(
                State(state.clone()),
                claims_for(owner_id),
                Path((workspace_id, member_id)),
                Json(UpdateMemberRoleRequest {
                    role: "admin".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(promoted["code"], 0, "{promoted}");
        let preview = body_json(to_response(
            post_flow_conversion_preview(State(state.clone()), claims_for(member_id), None, Json(preview_input()))
                .await,
        ))
        .await;
        assert_eq!(preview["code"], 0, "{preview}");
        let preview_id = Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");

        let demoted = body_json(to_response(
            update_member_role(
                State(state.clone()),
                claims_for(owner_id),
                Path((workspace_id, member_id)),
                Json(UpdateMemberRoleRequest {
                    role: "member".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(demoted["code"], 0, "{demoted}");
        let rejected = body_json(to_response(
            post_flow_conversion(
                State(state.clone()),
                claims_for(member_id),
                None,
                Json(crate::flow::bridge::ConversionCommitInput {
                    preview_id,
                    source_frontier: frontier,
                    target_schema_version: 0,
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(rejected["code"], 403, "{rejected}");
        let target_count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM project_forms WHERE project_id=$1 AND key LIKE 'converted_%'",
                vec![project_id.into()],
            ))
            .await
            .expect("target count query")
            .expect("target count");
        assert_eq!(target_count.try_get::<i64>("", "n").expect("count"), 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_bridge_conversion_commit_rechecks_flow_permission_through_production_feature_route() {
        use base64::Engine as _;

        #[derive(FromQueryResult)]
        struct Frontier {
            head_frontier: Vec<u8>,
        }

        let scratch = scratch_or_skip!("bridge-conversion-flow-shrink");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let enabled = body_json(to_response(
            set_flow_feature(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: Some(true),
                    default_member_level: Some("edit".to_string()),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(enabled["code"], 0, "{enabled}");

        let project_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, 'FSH', 'Flow shrink test', $3)",
            vec![project_id.into(), workspace_id.into(), owner_id.into()],
        )
        .await;
        let form = body_json(to_response(
            create_project_form(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(project_id),
                Json(CreateFormRequest {
                    key: "flow_shrink_target".to_string(),
                    name: "Flow shrink target".to_string(),
                    description: None,
                    icon: None,
                    color: None,
                    title_template: None,
                    schema: None,
                    detail_layout: None,
                }),
            )
            .await,
        ))
        .await;
        let form_id = Uuid::parse_str(form["data"]["id"].as_str().expect("form id")).expect("UUID");
        let forms_policy = body_json(to_response(
            update_form_permissions(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(form_id),
                Json(UpdateFormPermissionsRequest {
                    policies: vec![UpsertFormPermissionPolicy {
                        subject_type: "role".to_string(),
                        subject_id: "member".to_string(),
                        policy: json!({"actions":{"form.view":true,"record.create":true}}),
                    }],
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(forms_policy["code"], 0, "{forms_policy}");

        let source_id = create_page_as_owner(&state, workspace_id, owner_id, "flow shrink source").await;
        let head = Frontier::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE object_id=$1",
            vec![source_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("frontier query")
        .expect("frontier");
        let frontier = base64::engine::general_purpose::STANDARD.encode(head.head_frontier);
        let preview = body_json(to_response(
            post_flow_conversion_preview(
                State(state.clone()),
                claims_for(member_id),
                None,
                Json(crate::flow::bridge::ConversionPreviewInput {
                    source_object_id: source_id,
                    source_frontier: frontier.clone(),
                    target_type: "form_record".to_string(),
                    mapping: json!({"target_form_id":form_id,"values":{}}),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(preview["code"], 0, "{preview}");
        let preview_id = Uuid::parse_str(preview["data"]["preview_id"].as_str().expect("preview id")).expect("UUID");

        let narrowed = body_json(to_response(
            set_flow_feature(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: None,
                    default_member_level: Some("view".to_string()),
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(narrowed["code"], 0, "{narrowed}");
        let rejected = body_json(to_response(
            post_flow_conversion(
                State(state.clone()),
                claims_for(member_id),
                None,
                Json(crate::flow::bridge::ConversionCommitInput {
                    preview_id,
                    source_frontier: frontier.clone(),
                    target_schema_version: 1,
                    idempotency_key: Uuid::new_v4().to_string(),
                    confirm: true,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(rejected["code"], 403, "{rejected}");
        assert_eq!(
            rejected["message"], "bridge permission changed before commit",
            "the Flow shrink gate must reject before target reauthorization"
        );
        assert_eq!(
            rejected["details"]["permission_state"]["actions"],
            json!(["form.view"]),
            "Flow shrink must return the current intersection: {rejected}"
        );
        let zero = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT (SELECT count(*) FROM form_records WHERE form_id=$1) AS records, \
                        (SELECT count(*) FROM flow_object_lineage WHERE source_object_id=$2) AS lineage",
                vec![form_id.into(), source_id.into()],
            ))
            .await
            .expect("zero-write query")
            .expect("counts");
        assert_eq!(zero.try_get::<i64>("", "records").expect("records"), 0);
        assert_eq!(zero.try_get::<i64>("", "lineage").expect("lineage"), 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn member_mutation_without_flow_settings_succeeds_without_creating_them() {
        let scratch = scratch_or_skip!("member-no-flow-settings");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_bare_workspace(&state).await;
        let target_id = seed_user(&state).await;

        let added = body_json(to_response(
            add_member(
                State(state.clone()),
                claims_for(owner_id),
                Path(workspace_id),
                Json(AddMemberRequest {
                    user_id: target_id,
                    role: "member".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(added["code"], 0, "{added}");

        let settings_count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_workspace_settings WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("settings count query runs")
            .expect("settings count exists");
        assert_eq!(settings_count.try_get::<i64>("", "n").expect("count reads"), 0);

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn bot_creation_member_write_advances_existing_flow_epoch() {
        let scratch = scratch_or_skip!("bot-member-epoch");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;

        let created = body_json(to_response(
            create_bot(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(CreateBotRequest {
                    name: "epoch bot".to_string(),
                    permissions: Some(vec!["read".to_string()]),
                    transport_surface: None,
                    expires_at: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(created["code"], 0, "{created}");
        assert_eq!(read_epoch(&state, workspace_id).await, 1);

        scratch.drop_self().await;
    }

    /// `POST /api/v1/flow/objects/{object_id}/commands`: all eight v0.4 command types, each
    /// exercised at least once against a real database and the real shared write path
    /// (`flow::command::execute_content_command` calls the identical `write::accept_update`
    /// `flow::collab::session` uses), plus two independent error paths — an `expected_frontier`
    /// mismatch (`stale_frontier`) and an unregistered `command.type` (`invalid_update`) — both
    /// surfaced through the envelope `code`, never the HTTP transport status.
    #[tokio::test]
    #[allow(clippy::items_after_statements)]
    async fn commands_endpoint_covers_all_seven_v04_types_and_two_error_paths() {
        let scratch = scratch_or_skip!("commands-all-types");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let create_response = to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Commands Test Page".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        );
        let create_body = body_json(create_response).await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id = Uuid::parse_str(
            create_body["data"]["object"]["id"]
                .as_str()
                .expect("object id is a string"),
        )
        .expect("object id is a uuid");

        async fn run_command(
            state: &AppState,
            claims: &Extension<JwtClaims>,
            object_id: Uuid,
            command_type: &str,
            payload: Value,
            expected_frontier: Option<String>,
        ) -> Value {
            let response = to_response(
                post_flow_object_command(
                    State(state.clone()),
                    claims.clone(),
                    None,
                    Path(object_id),
                    Json(ExecuteFlowCommandRequest {
                        command: FlowCommandEnvelope {
                            command_type: command_type.to_string(),
                            payload,
                        },
                        expected_frontier,
                        idempotency_key: Uuid::new_v4().to_string(),
                        message: Some(format!("e2e {command_type}")),
                    }),
                )
                .await,
            );
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            body_json(response).await
        }

        // 1. set_title
        let body = run_command(
            &state,
            &claims,
            object_id,
            "set_title",
            json!({"title": "Renamed"}),
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["object"]["title"], "Renamed");
        assert_eq!(body["data"]["accepted_seq"], 1);

        // 2. insert_block
        let body = run_command(
            &state,
            &claims,
            object_id,
            "insert_block",
            json!({"block_id": "blk-1", "index": 0, "text": "Hello"}),
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["accepted_seq"], 2);

        // 3. update_block
        let body = run_command(
            &state,
            &claims,
            object_id,
            "update_block",
            json!({"block_id": "blk-1", "text": "Hello world"}),
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["accepted_seq"], 3);

        // 4. move_block
        let body = run_command(
            &state,
            &claims,
            object_id,
            "move_block",
            json!({"block_id": "blk-1", "index": 0}),
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["accepted_seq"], 4);

        // 5. delete_block
        let body = run_command(
            &state,
            &claims,
            object_id,
            "delete_block",
            json!({"block_id": "blk-1"}),
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["accepted_seq"], 5);

        // 6. archive
        let body = run_command(&state, &claims, object_id, "archive", json!({}), None).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["object"]["lifecycle_status"], "archived");
        assert!(body["data"]["object"]["archived_at"].is_string(), "{body}");

        // 7. restore -- idempotent lifecycle transition back to active.
        let body = run_command(&state, &claims, object_id, "restore", json!({}), None).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["object"]["lifecycle_status"], "active");
        assert!(body["data"]["object"]["archived_at"].is_null(), "{body}");

        // ---- error path 1: `expected_frontier` does not match the real current frontier ----
        // `stale_frontier` -> `ApiError::Conflict` -> envelope `code = 409`.
        let bogus_frontier = base64::engine::general_purpose::STANDARD.encode(b"not-the-real-frontier");
        let body = run_command(
            &state,
            &claims,
            object_id,
            "set_title",
            json!({"title": "Must not apply"}),
            Some(bogus_frontier),
        )
        .await;
        assert_eq!(
            body["code"], 409,
            "a stale expected_frontier must surface as body code 409: {body}"
        );
        let get_after_stale = to_response(
            get_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        );
        let get_after_stale_body = body_json(get_after_stale).await;
        assert_ne!(
            get_after_stale_body["data"]["title"], "Must not apply",
            "the rejected write must not have been applied: {get_after_stale_body}"
        );

        // ---- error path 2: an unregistered command.type ----
        // `invalid_update` -> `ApiError::BadRequest` -> envelope `code = 400`.
        let body = run_command(&state, &claims, object_id, "not_a_real_command", json!({}), None).await;
        assert_eq!(
            body["code"], 400,
            "an unregistered command type must surface as body code 400: {body}"
        );

        scratch.drop_self().await;
    }

    /// WP-09's decisive branches against real accepted history: seq-zero retains the creation
    /// title, semantic JSON contains logical nodes but no update/peer representation, markdown
    /// comes back on demand, all three range failures are typed and never clamped, and a policy
    /// boundary makes an existing object indistinguishable from an absent one.
    #[tokio::test]
    #[allow(clippy::items_after_statements)]
    async fn diff_is_semantic_range_strict_and_policy_collapsed() {
        let scratch = scratch_or_skip!("diff-contract");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let claims = claims_for(owner_id);
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "Created at seq zero").await;

        async fn command(
            state: &AppState,
            claims: &Extension<JwtClaims>,
            object_id: Uuid,
            command_type: &str,
            payload: Value,
        ) -> Value {
            body_json(to_response(
                post_flow_object_command(
                    State(state.clone()),
                    claims.clone(),
                    None,
                    Path(object_id),
                    Json(ExecuteFlowCommandRequest {
                        command: FlowCommandEnvelope {
                            command_type: command_type.to_string(),
                            payload,
                        },
                        expected_frontier: None,
                        idempotency_key: Uuid::new_v4().to_string(),
                        message: None,
                    }),
                )
                .await,
            ))
            .await
        }

        let renamed = command(
            &state,
            &claims,
            object_id,
            "set_title",
            json!({"title": "Current title"}),
        )
        .await;
        assert_eq!(renamed["data"]["accepted_seq"], 1, "{renamed}");
        let inserted = command(
            &state,
            &claims,
            object_id,
            "insert_block",
            json!({"block_id": "logical-block", "index": 0, "text": "semantic text"}),
        )
        .await;
        assert_eq!(inserted["data"]["accepted_seq"], 2, "{inserted}");

        let diff = body_json(to_response(
            get_flow_object_diff(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(FlowObjectDiffQuery {
                    from_seq: Some(0),
                    to_seq: Some(2),
                    render: Some("markdown".to_string()),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(diff["code"], 0, "{diff}");
        assert_eq!(diff["data"]["from_seq"], 0);
        assert_eq!(diff["data"]["to_seq"], 2);
        assert_eq!(diff["data"]["semantic_diff"]["title"]["before"], "Created at seq zero");
        assert_eq!(diff["data"]["semantic_diff"]["title"]["after"], "Current title");
        assert_eq!(
            diff["data"]["semantic_diff"]["nodes"]["added"]["logical-block"]["text"],
            "semantic text"
        );
        assert_eq!(diff["data"]["rendered"], "# Current title\n\nsemantic text\n");
        assert!(
            diff["data"]["rendered"]
                .as_str()
                .is_some_and(|markdown| markdown.contains("semantic text")),
            "markdown dropped the accepted block body: {diff}"
        );
        let serialized = diff["data"].to_string();
        assert!(!serialized.contains("bytes"), "diff leaked update bytes: {serialized}");
        assert!(
            !serialized.contains("peer_id"),
            "diff leaked an engine peer id: {serialized}"
        );

        for (from_seq, to_seq) in [(2, 1), (-1, 1)] {
            let rejected = body_json(to_response(
                get_flow_object_diff(
                    State(state.clone()),
                    claims.clone(),
                    None,
                    Path(object_id),
                    Query(FlowObjectDiffQuery {
                        from_seq: Some(from_seq),
                        to_seq: Some(to_seq),
                        render: None,
                    }),
                )
                .await,
            ))
            .await;
            assert_eq!(
                rejected["error_code"], "invalid_update",
                "range was silently clamped: {rejected}"
            );
        }
        let beyond_head = body_json(to_response(
            get_flow_object_diff(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(FlowObjectDiffQuery {
                    from_seq: Some(0),
                    to_seq: Some(3),
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(beyond_head["error_code"], "stale_frontier", "{beyond_head}");
        assert_eq!(beyond_head["details"]["current_seq"], 2, "{beyond_head}");

        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![object_id.into()],
        )
        .await;
        let denied = body_json(to_response(
            get_flow_object_diff(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(object_id),
                Query(FlowObjectDiffQuery {
                    from_seq: Some(0),
                    to_seq: Some(0),
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        let absent = body_json(to_response(
            get_flow_object_diff(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(Uuid::new_v4()),
                Query(FlowObjectDiffQuery {
                    from_seq: Some(0),
                    to_seq: Some(0),
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(denied["code"], 404, "{denied}");
        assert_eq!(absent["code"], 404, "{absent}");
        assert_eq!(denied["message"], absent["message"]);

        // Malformed query values must not run before object-level authorization. Otherwise the
        // 400 response itself proves that a guessed UUID names a real object. Exercise both
        // malformed branches against an absent id, a real object in another workspace, and a
        // same-workspace object behind an inheritance boundary; compare complete JSON envelopes,
        // not merely status codes.
        let (other_workspace_id, other_owner_id) = seed_workspace(&state, true).await;
        let other_workspace_object =
            create_page_as_owner(&state, other_workspace_id, other_owner_id, "other tenant").await;
        for malformed in [
            FlowObjectDiffQuery {
                from_seq: None,
                to_seq: Some(0),
                render: None,
            },
            FlowObjectDiffQuery {
                from_seq: Some(0),
                to_seq: Some(0),
                render: Some("not-a-renderer".to_string()),
            },
        ] {
            let mut responses = Vec::new();
            for target in [Uuid::new_v4(), other_workspace_object, object_id] {
                responses.push(
                    body_json(to_response(
                        get_flow_object_diff(
                            State(state.clone()),
                            claims_for(member_id),
                            None,
                            Path(target),
                            Query(FlowObjectDiffQuery {
                                from_seq: malformed.from_seq,
                                to_seq: malformed.to_seq,
                                render: malformed.render.clone(),
                            }),
                        )
                        .await,
                    ))
                    .await,
                );
            }
            assert_eq!(responses[0], responses[1], "cross-tenant object existence leaked");
            assert_eq!(
                responses[0], responses[2],
                "same-workspace hidden object existence leaked"
            );
            assert_eq!(responses[0]["code"], 404, "{responses:?}");
        }

        // Hit the storage invariant itself: removing seq 1 leaves seq 2 reachable to a naive
        // reader, but the endpoint must reject the requested history instead of skipping/clamping.
        let document_id = document_of(&state, object_id).await;
        exec(
            &state,
            "DELETE FROM collab_updates WHERE document_id = $1 AND seq = 1",
            vec![document_id.into()],
        )
        .await;
        let missing_seq = body_json(to_response(
            get_flow_object_diff(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(FlowObjectDiffQuery {
                    from_seq: Some(0),
                    to_seq: Some(2),
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(missing_seq["error_code"], "resync_required", "{missing_seq}");

        scratch.drop_self().await;
    }

    /// The endpoint and storage query must enforce the same one-past boundary. A pure helper test
    /// cannot prove that SQL fetched row 1001; this real history fixture does. If `LIMIT` is
    /// mutated from `row_limit + 1` to `row_limit`, the endpoint sees only 1000 rows and falls
    /// through to a misleading `resync_required` instead of this typed budget rejection.
    #[tokio::test]
    async fn diff_endpoint_rejects_real_1001_row_history_as_limit_exceeded() {
        let scratch = scratch_or_skip!("diff-row-budget");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "large history").await;
        let document_id = document_of(&state, object_id).await;

        exec(
            &state,
            "INSERT INTO collab_updates \
                (document_id, seq, update_id, content_hash, before_frontier, after_frontier, \
                 bytes, actor_id, origin_surface, origin_client_id, projection_seq, event_id) \
             SELECT $1, n, gen_random_uuid(), lpad(n::text, 64, '0'), ''::bytea, ''::bytea, \
                    ''::bytea, $2, 'rest', 'diff-budget-fixture', n, \
                    (SELECT id FROM business_events WHERE aggregate_id = $3 ORDER BY id LIMIT 1) \
               FROM generate_series(1, 1001) AS n",
            vec![document_id.into(), owner_id.into(), object_id.to_string().into()],
        )
        .await;
        exec(
            &state,
            "UPDATE collab_documents \
                SET head_seq = 1001, update_count = 1001, head_frontier = ''::bytea \
              WHERE id = $1",
            vec![document_id.into()],
        )
        .await;

        let response = body_json(to_response(
            get_flow_object_diff(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_id),
                Query(FlowObjectDiffQuery {
                    from_seq: Some(0),
                    to_seq: Some(1001),
                    render: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(response["error_code"], "limit_exceeded", "{response}");
        assert_eq!(response["details"]["limit_kind"], "scan_budget", "{response}");
        assert_eq!(response["details"]["limit"], 1000, "{response}");
        assert_eq!(response["details"]["observed"], 1001, "{response}");

        scratch.drop_self().await;
    }

    /// WP-12's side-channel regression fixture deliberately puts a no-grant authorization
    /// boundary on the highest-lag object. It also places that hidden candidate between the last
    /// returned object and the overfetched next visible object, so a cursor derived from internal
    /// scan progress (instead of the actual returned row) makes page two skip data and fail.
    #[tokio::test]
    #[allow(clippy::items_after_statements)]
    async fn projection_lag_filters_before_aggregating_and_cursors_from_returned_rows() {
        let scratch = scratch_or_skip!("projection-lag-contract");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;

        let visible_a = create_page_as_owner(&state, workspace_id, owner_id, "visible a").await;
        let visible_b = create_page_as_owner(&state, workspace_id, owner_id, "visible b").await;
        let hidden = create_page_as_owner(&state, workspace_id, owner_id, "hidden huge lag").await;
        let visible_c = create_page_as_owner(&state, workspace_id, owner_id, "visible c").await;

        async fn set_lag(state: &AppState, object_id: Uuid, head_seq: i64, projection_seq: i64, ordinal: i64) {
            exec(
                state,
                "UPDATE collab_documents SET head_seq = $2 WHERE object_id = $1",
                vec![object_id.into(), head_seq.into()],
            )
            .await;
            exec(
                state,
                "UPDATE flow_object_projections SET document_seq = $2 WHERE object_id = $1",
                vec![object_id.into(), projection_seq.into()],
            )
            .await;
            exec(
                state,
                "UPDATE flow_objects SET created_at = TIMESTAMPTZ '2026-01-01 00:00:00+00' + make_interval(secs => $2::int) WHERE id = $1",
                vec![object_id.into(), ordinal.into()],
            )
            .await;
        }

        set_lag(&state, visible_a, 5, 3, 1).await;
        set_lag(&state, visible_b, 10, 3, 2).await;
        set_lag(&state, hidden, 10_000, 0, 3).await;
        set_lag(&state, visible_c, 9, 8, 4).await;
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![hidden.into()],
        )
        .await;

        let first = body_json(to_response(
            get_flow_projection_lag(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ProjectionLagQuery {
                    project_id: None,
                    cursor: None,
                    limit: Some(2),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(first["code"], 0, "{first}");
        let first_data = first["data"].as_object().expect("response data is an object");
        let first_keys: std::collections::BTreeSet<&str> = first_data.keys().map(String::as_str).collect();
        assert_eq!(
            first_keys,
            ["items", "max_lag", "next_cursor", "p95_lag"].into_iter().collect(),
            "pre-filter totals/counts must not become response fields: {first}"
        );
        assert_eq!(first["data"]["max_lag"], 7, "hidden lag leaked into max: {first}");
        assert_eq!(first["data"]["p95_lag"], 7, "hidden lag leaked into p95: {first}");
        assert_eq!(first["data"]["items"].as_array().expect("items").len(), 2);
        assert_eq!(first["data"]["items"][0]["object_id"], visible_a.to_string());
        assert_eq!(first["data"]["items"][0]["head_seq"], 5);
        assert_eq!(first["data"]["items"][0]["projection_seq"], 3);
        assert_eq!(first["data"]["items"][0]["lag"], 2);
        assert_eq!(first["data"]["items"][1]["object_id"], visible_b.to_string());
        assert_eq!(first["data"]["items"][1]["lag"], 7);
        let serialized = first["data"].to_string();
        let hidden_text = hidden.to_string();
        for forbidden in [
            "content",
            "bytes",
            "total",
            "filtered_count",
            "examined",
            hidden_text.as_str(),
        ] {
            assert!(
                !serialized.contains(forbidden),
                "projection lag leaked `{forbidden}`: {serialized}"
            );
        }

        let cursor = first["data"]["next_cursor"]
            .as_str()
            .expect("overfetch found another visible row")
            .to_string();
        let second = body_json(to_response(
            get_flow_projection_lag(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ProjectionLagQuery {
                    project_id: None,
                    cursor: Some(cursor),
                    limit: Some(2),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(second["code"], 0, "{second}");
        assert_eq!(second["data"]["items"].as_array().expect("items").len(), 1, "{second}");
        assert_eq!(
            second["data"]["items"][0]["object_id"],
            visible_c.to_string(),
            "{second}"
        );
        assert_eq!(
            second["data"]["max_lag"], 7,
            "the policy-filtered scope aggregate must not jump while paging: {second}"
        );
        assert_eq!(
            second["data"]["p95_lag"], 7,
            "the policy-filtered scope aggregate must not jump while paging: {second}"
        );
        assert!(second["data"]["next_cursor"].is_null(), "{second}");

        let over_limit = body_json(to_response(
            get_flow_projection_lag(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ProjectionLagQuery {
                    project_id: None,
                    cursor: None,
                    limit: Some(101),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            over_limit["error_code"], "limit_exceeded",
            "limit was silently clamped: {over_limit}"
        );
        assert_eq!(over_limit["details"]["limit"], 100, "{over_limit}");
        assert_eq!(over_limit["details"]["observed"], 101, "{over_limit}");

        scratch.drop_self().await;
    }

    /// Scope aggregation and item pagination have different work shapes. More than 1000 visible
    /// objects must not make page one fail merely because the aggregate covers the whole scope;
    /// the aggregate stays scope-wide in SQL while the item scan advances from the caller cursor
    /// and stops after `limit + 1` visible rows.
    #[tokio::test]
    async fn projection_lag_large_scope_remains_usable_and_pageable() {
        let scratch = scratch_or_skip!("projection-lag-large-scope");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let source = create_page_as_owner(&state, workspace_id, owner_id, "aggregate seed").await;
        let root_id = crate::flow::repository::fetch_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("root lookup runs")
            .expect("creating the aggregate seed materializes the root");

        exec(
            &state,
            "CREATE TABLE flow_projection_lag_bulk_ids AS \
             SELECT gen_random_uuid() AS id, n \
               FROM generate_series(1, 1001) AS n",
            Vec::new(),
        )
        .await;
        exec(
            &state,
            "INSERT INTO flow_objects \
                (id, workspace_id, object_type, parent_id, created_by, updated_by, created_at, updated_at) \
             SELECT id, $1, 'page', $3, $2, $2, \
                    TIMESTAMPTZ '2026-01-01 00:00:00+00' + make_interval(secs => n), \
                    TIMESTAMPTZ '2026-01-01 00:00:00+00' + make_interval(secs => n) \
               FROM flow_projection_lag_bulk_ids",
            vec![workspace_id.into(), owner_id.into(), root_id.into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO collab_documents \
                (object_id, format_version, snapshot, snapshot_frontier, head_seq, \
                 head_frontier) \
             SELECT b.id, d.format_version, d.snapshot, d.snapshot_frontier, b.n % 17, \
                    d.head_frontier \
               FROM flow_projection_lag_bulk_ids b \
               CROSS JOIN collab_documents d \
              WHERE d.object_id = $1",
            vec![source.into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO flow_object_projections \
                (object_id, document_seq, document_frontier, title, plain_text) \
             SELECT id, 0, ''::bytea, 'bulk projection lag', '' \
               FROM flow_projection_lag_bulk_ids",
            Vec::new(),
        )
        .await;

        let first = body_json(to_response(
            get_flow_projection_lag(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ProjectionLagQuery {
                    project_id: None,
                    cursor: None,
                    limit: Some(50),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(first["code"], 0, "large-scope first page failed: {first}");
        assert_eq!(first["data"]["items"].as_array().expect("items").len(), 50);
        assert_eq!(first["data"]["max_lag"], 16, "{first}");
        assert_eq!(first["data"]["p95_lag"], 16, "{first}");
        let cursor = first["data"]["next_cursor"]
            .as_str()
            .expect("large scope has a second page")
            .to_string();

        let second = body_json(to_response(
            get_flow_projection_lag(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(ProjectionLagQuery {
                    project_id: None,
                    cursor: Some(cursor),
                    limit: Some(50),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(second["code"], 0, "large-scope second page failed: {second}");
        assert_eq!(second["data"]["items"].as_array().expect("items").len(), 50);
        assert_eq!(second["data"]["max_lag"], first["data"]["max_lag"]);
        assert_eq!(second["data"]["p95_lag"], first["data"]["p95_lag"]);
        assert!(second["data"]["next_cursor"].is_string(), "{second}");

        scratch.drop_self().await;
    }

    async fn index_accepted_projection(state: &AppState, object_id: Uuid) {
        exec(
            state,
            "INSERT INTO flow_search_index (object_id, indexed_seq, indexed_frontier, title, plain_text) \
             SELECT object_id, document_seq, document_frontier, title, plain_text \
               FROM flow_object_projections WHERE object_id = $1",
            vec![object_id.into()],
        )
        .await;
    }

    fn search_query(q: &str) -> FlowSearchQuery {
        FlowSearchQuery {
            q: q.to_string(),
            project_id: None,
            unprojected: false,
            all_visible: true,
            object_type: None,
            freshness: None,
            cursor: None,
            limit: None,
        }
    }

    /// Hidden candidates sit between two visible hits in rank order. The test asserts directly
    /// on both returned pages and their wire keys, so a pre-authorization total/cursor or a
    /// reader that applies a second filter cannot hide the regression.
    #[tokio::test]
    async fn flow_search_filters_before_cardinality_cursor_snippet_and_frontier() {
        let scratch = scratch_or_skip!("flow-search-policy");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let visible_a = create_page_as_owner(&state, workspace_id, owner_id, "needle needle needle").await;
        let hidden = create_page_as_owner(&state, workspace_id, owner_id, "needle needle hidden-secret").await;
        let visible_b = create_page_as_owner(&state, workspace_id, owner_id, "needle visible-last").await;
        for object_id in [visible_a, hidden, visible_b] {
            index_accepted_projection(&state, object_id).await;
        }
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![hidden.into()],
        )
        .await;

        let mut first_query = search_query("needle");
        first_query.limit = Some(1);
        let first = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(first_query),
            )
            .await,
        ))
        .await;
        assert_eq!(first["code"], 0, "{first}");
        let data = first["data"].as_object().expect("search data is an object");
        assert_eq!(
            data.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            ["index_frontier", "items", "next_cursor"].into_iter().collect(),
            "no pre-filter count may enter the response: {first}"
        );
        assert_eq!(first["data"]["items"].as_array().expect("items").len(), 1);
        assert_eq!(first["data"]["items"][0]["object"]["id"], visible_a.to_string());
        assert_eq!(first["data"]["index_frontier"]["stale"], false);
        let cursor = first["data"]["next_cursor"]
            .as_str()
            .expect("another visible hit exists")
            .to_string();
        let cursor_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&cursor)
            .expect("cursor is base64url");
        let cursor_text = String::from_utf8_lossy(&cursor_bytes);
        for forbidden in [
            visible_a.to_string(),
            visible_b.to_string(),
            hidden.to_string(),
            "needle".to_string(),
        ] {
            assert!(
                !cursor_text.contains(&forbidden),
                "cursor leaked `{forbidden}`: {cursor_text}"
            );
        }

        let mut second_query = search_query("needle");
        second_query.limit = Some(1);
        second_query.cursor = Some(cursor);
        let second = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(second_query),
            )
            .await,
        ))
        .await;
        assert_eq!(second["code"], 0, "{second}");
        assert_eq!(second["data"]["items"].as_array().expect("items").len(), 1);
        assert_eq!(second["data"]["items"][0]["object"]["id"], visible_b.to_string());
        assert!(second["data"]["next_cursor"].is_null(), "{second}");
        let serialized = format!("{first}{second}");
        for forbidden in [
            hidden.to_string(),
            "hidden-secret".to_string(),
            "total".to_string(),
            "examined".to_string(),
        ] {
            assert!(
                !serialized.contains(&forbidden),
                "search leaked `{forbidden}`: {serialized}"
            );
        }

        scratch.drop_self().await;
    }

    /// Frontier computation is a database aggregate, not candidate overfetch. A large active
    /// scope with one actual full-text match must remain searchable; applying the 1,000-row
    /// overfetch budget to every object in the scope makes this fixture fail with
    /// `limit_exceeded` before it can return the one hit.
    #[tokio::test]
    async fn flow_search_large_scope_with_one_match_does_not_spend_candidate_scan_budget_on_frontier() {
        let scratch = scratch_or_skip!("flow-search-large-scope");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let matching = create_page_as_owner(&state, workspace_id, owner_id, "unique-frontier-needle").await;
        index_accepted_projection(&state, matching).await;
        let root_id = crate::flow::repository::fetch_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("root lookup runs")
            .expect("creating the matching page materializes the root");

        exec(
            &state,
            "CREATE TABLE flow_search_bulk_ids AS \
             SELECT gen_random_uuid() AS id FROM generate_series(1, $1::int)",
            vec![1_001_i64.into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO flow_objects (id, workspace_id, object_type, parent_id, created_by, updated_by) \
             SELECT id, $1, 'page', $3, $2, $2 FROM flow_search_bulk_ids",
            vec![workspace_id.into(), owner_id.into(), root_id.into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO collab_documents \
                (object_id, format_version, snapshot, snapshot_frontier, head_frontier) \
             SELECT b.id, d.format_version, d.snapshot, d.snapshot_frontier, d.head_frontier \
               FROM flow_search_bulk_ids b \
               CROSS JOIN collab_documents d \
              WHERE d.object_id = $1",
            vec![matching.into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO flow_object_projections \
                (object_id, document_seq, document_frontier, title, plain_text) \
             SELECT id, 0, $1, 'filler object', 'does not match the query' \
               FROM flow_search_bulk_ids",
            vec![Vec::<u8>::new().into()],
        )
        .await;
        exec(
            &state,
            "INSERT INTO flow_search_index \
                (object_id, indexed_seq, indexed_frontier, title, plain_text) \
             SELECT object_id, document_seq, document_frontier, title, plain_text \
               FROM flow_object_projections \
              WHERE object_id IN (SELECT id FROM flow_search_bulk_ids)",
            Vec::new(),
        )
        .await;

        let response = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(search_query("unique-frontier-needle")),
            )
            .await,
        ))
        .await;
        assert_eq!(response["code"], 0, "{response}");
        assert_eq!(response["data"]["items"].as_array().expect("items").len(), 1);
        assert_eq!(response["data"]["items"][0]["object"]["id"], matching.to_string());

        scratch.drop_self().await;
    }

    /// HTML snippets must encode source text injectively. If ampersands are left untouched, the
    /// literal text `&lt;img&gt;` and a real `<img>` element collapse to the same wire bytes; an HTML
    /// renderer then decodes attacker-controlled markup from an apparently escaped snippet.
    #[tokio::test]
    async fn flow_search_snippet_distinguishes_literal_entity_text_from_real_markup() {
        let scratch = scratch_or_skip!("flow-search-snippet-escape");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let literal = create_page_as_owner(&state, workspace_id, owner_id, "shared &lt;img&gt; suffix").await;
        let markup = create_page_as_owner(&state, workspace_id, owner_id, "shared <img> suffix").await;
        for object_id in [literal, markup] {
            index_accepted_projection(&state, object_id).await;
        }

        let response = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(search_query("shared")),
            )
            .await,
        ))
        .await;
        assert_eq!(response["code"], 0, "{response}");
        let items = response["data"]["items"].as_array().expect("items");
        assert_eq!(items.len(), 2, "both fixtures must match: {response}");
        let snippet_for = |object_id: Uuid| {
            items
                .iter()
                .find(|item| item["object"]["id"] == object_id.to_string())
                .and_then(|item| item["snippets"]["title"].as_str())
                .unwrap_or_else(|| panic!("missing title snippet for {object_id}: {response}"))
        };
        let literal_snippet = snippet_for(literal);
        let markup_snippet = snippet_for(markup);
        assert_ne!(
            literal_snippet, markup_snippet,
            "literal entity text and real markup must not collapse to identical HTML"
        );
        assert!(literal_snippet.contains("&amp;lt;"), "{literal_snippet}");
        assert!(markup_snippet.contains("&lt;"), "{markup_snippet}");
        assert!(!literal_snippet.contains("<img"), "{literal_snippet}");
        assert!(!markup_snippet.contains("<img"), "{markup_snippet}");

        scratch.drop_self().await;
    }

    /// The visible object's old accepted index remains the only source of title/snippet while
    /// lagging. `require_current` refuses the same scope, and a stale no-grant object cannot make
    /// a member's policy-filtered frontier stale.
    #[tokio::test]
    async fn flow_search_stale_projection_is_explicit_and_require_current_fails_closed() {
        let scratch = scratch_or_skip!("flow-search-stale");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let visible = create_page_as_owner(&state, workspace_id, owner_id, "old accepted needle").await;
        let hidden = create_page_as_owner(&state, workspace_id, owner_id, "hidden old needle").await;
        for object_id in [visible, hidden] {
            index_accepted_projection(&state, object_id).await;
            exec(
                &state,
                "UPDATE collab_documents SET head_seq = 1, head_frontier = $2 WHERE object_id = $1",
                vec![object_id.into(), vec![1_u8].into()],
            )
            .await;
            exec(
                &state,
                "UPDATE flow_object_projections SET document_seq = 1, document_frontier = $2, \
                 title = 'new projection without the query', plain_text = 'new accepted body' WHERE object_id = $1",
                vec![object_id.into(), vec![1_u8].into()],
            )
            .await;
        }
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![hidden.into()],
        )
        .await;

        let allowed = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(search_query("needle")),
            )
            .await,
        ))
        .await;
        assert_eq!(allowed["code"], 0, "{allowed}");
        assert_eq!(
            allowed["data"]["items"].as_array().expect("items").len(),
            1,
            "{allowed}"
        );
        assert_eq!(allowed["data"]["items"][0]["object"]["id"], visible.to_string());
        assert_eq!(allowed["data"]["items"][0]["object"]["title"], "old accepted needle");
        assert_eq!(allowed["data"]["items"][0]["indexed_seq"], 0);
        assert_eq!(allowed["data"]["items"][0]["head_seq"], 1);
        assert_eq!(allowed["data"]["items"][0]["projection_lag"], 1);
        assert_eq!(allowed["data"]["items"][0]["stale"], true);
        assert_eq!(allowed["data"]["index_frontier"]["indexed_seq"], 0);
        assert_eq!(allowed["data"]["index_frontier"]["head_seq"], 1);
        assert_eq!(allowed["data"]["index_frontier"]["lag"], 1);
        assert_eq!(allowed["data"]["index_frontier"]["stale"], true);
        assert!(!allowed.to_string().contains("hidden old needle"), "{allowed}");
        assert!(!allowed.to_string().contains("new accepted body"), "{allowed}");

        let mut current = search_query("needle");
        current.freshness = Some("require_current".to_string());
        let member_current = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(current),
            )
            .await,
        ))
        .await;
        assert_eq!(member_current["error_code"], "stale_frontier", "{member_current}");

        // Make the visible row current. The hidden row stays stale, but cannot affect the
        // member's frontier. The owner can see it and therefore still fails closed.
        exec(
            &state,
            "UPDATE flow_search_index si SET indexed_seq = p.document_seq, indexed_frontier = p.document_frontier, \
             title = p.title, plain_text = p.plain_text FROM flow_object_projections p \
             WHERE si.object_id = p.object_id AND si.object_id = $1",
            vec![visible.into()],
        )
        .await;
        let mut member_fresh = search_query("accepted");
        member_fresh.freshness = Some("require_current".to_string());
        let member_fresh = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(member_id),
                None,
                Path(workspace_id),
                Query(member_fresh),
            )
            .await,
        ))
        .await;
        assert_eq!(
            member_fresh["code"], 0,
            "hidden lag altered the visible frontier: {member_fresh}"
        );
        assert_eq!(member_fresh["data"]["index_frontier"]["stale"], false);

        let mut owner_current = search_query("accepted");
        owner_current.freshness = Some("require_current".to_string());
        let owner_current = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(owner_current),
            )
            .await,
        ))
        .await;
        assert_eq!(owner_current["error_code"], "stale_frontier", "{owner_current}");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_search_rejects_each_invalid_scope_and_bot_all_visible_but_allows_bot_single_scope() {
        let scratch = scratch_or_skip!("flow-search-scope");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "bot searchable needle").await;
        index_accepted_projection(&state, object_id).await;

        let none = FlowSearchQuery {
            all_visible: false,
            ..search_query("needle")
        };
        let none = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(none),
            )
            .await,
        ))
        .await;
        assert_eq!(none["error_code"], "invalid_update", "{none}");

        let both = FlowSearchQuery {
            project_id: Some(Uuid::new_v4()),
            unprojected: true,
            all_visible: false,
            ..search_query("needle")
        };
        let both = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(both),
            )
            .await,
        ))
        .await;
        assert_eq!(both["error_code"], "invalid_update", "{both}");

        let bot = Extension(crate::middleware::bot_auth::BotAuthContext {
            bot_id: Uuid::new_v4(),
            workspace_id,
            permissions: vec!["read".to_string()],
            surface: crate::flow::event_origin::EventSurface::McpHttp,
            tool_name: Some("objects.search".to_string()),
            request_id: Uuid::new_v4(),
        });
        let bot_all = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                Some(bot.clone()),
                Path(workspace_id),
                Query(search_query("needle")),
            )
            .await,
        ))
        .await;
        assert_eq!(bot_all["code"], 403, "{bot_all}");

        let bot_single = FlowSearchQuery {
            unprojected: true,
            all_visible: false,
            ..search_query("needle")
        };
        let bot_single = body_json(to_response(
            get_flow_search(
                State(state.clone()),
                claims_for(owner_id),
                Some(bot),
                Path(workspace_id),
                Query(bot_single),
            )
            .await,
        ))
        .await;
        assert_eq!(bot_single["code"], 0, "{bot_single}");
        assert_eq!(bot_single["data"]["items"].as_array().expect("items").len(), 1);

        scratch.drop_self().await;
    }

    /// `GET /api/v1/flow/objects/{object_id}/bootstrap`: a user request returns the complete
    /// `Bootstrap` shape (`snapshot_base64`/`tail_updates`/`head_frontier`/`limits`/
    /// `websocket_path`) with the real document identity; a bot token is rejected outright
    /// (`rest-api-v1.md`: "**user only**").
    #[tokio::test]
    async fn bootstrap_endpoint_returns_the_full_shape_for_a_user_and_rejects_a_bot() {
        let scratch = scratch_or_skip!("bootstrap-basic");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Bootstrap Test Page".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        let object_id = Uuid::parse_str(
            create_body["data"]["object"]["id"]
                .as_str()
                .expect("object id is a string"),
        )
        .expect("object id is a uuid");
        let document_id = Uuid::parse_str(
            create_body["data"]["object"]["document_id"]
                .as_str()
                .expect("document id is a string"),
        )
        .expect("document id is a uuid");

        let response = to_response(
            get_flow_object_bootstrap(
                State(state.clone()),
                claims.clone(),
                None,
                Path(object_id),
                Query(GetFlowObjectBootstrapQuery {
                    known_seq: None,
                    known_frontier: None,
                }),
            )
            .await,
        );
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["object_id"], object_id.to_string());
        assert_eq!(body["data"]["document_id"], document_id.to_string());
        assert_eq!(body["data"]["engine"], "loro");
        assert_eq!(body["data"]["snapshot_seq"], 0);
        assert_eq!(body["data"]["head_seq"], 0);
        assert!(body["data"]["snapshot_base64"].is_string(), "{body}");
        assert!(!body["data"]["snapshot_base64"].as_str().unwrap().is_empty(), "{body}");
        assert!(body["data"]["tail_updates"].as_array().unwrap().is_empty(), "{body}");
        assert!(body["data"]["head_frontier"].is_string(), "{body}");
        assert_eq!(body["data"]["websocket_path"], "/api/v1/collab/ws");
        let limits = &body["data"]["limits"];
        assert_eq!(limits["version"], "sylvode.flow.limits.v1", "{body}");
        assert_eq!(limits["update_bytes_max"], 65_536, "{body}");
        assert_eq!(limits["bootstrap_decoded_bytes_max"], 8_388_608, "{body}");
        assert_eq!(limits["import_compression_ratio_max"], 100, "{body}");

        // A bot token must never reach the bootstrap handler's actual logic.
        let bot_ctx = Extension(crate::middleware::bot_auth::BotAuthContext {
            bot_id: Uuid::new_v4(),
            workspace_id,
            permissions: vec!["read".to_string(), "write".to_string(), "admin".to_string()],
            surface: crate::flow::event_origin::EventSurface::Rest,
            tool_name: None,
            request_id: uuid::Uuid::new_v4(),
        });
        let bot_response = to_response(
            get_flow_object_bootstrap(
                State(state.clone()),
                claims.clone(),
                Some(bot_ctx),
                Path(object_id),
                Query(GetFlowObjectBootstrapQuery {
                    known_seq: None,
                    known_frontier: None,
                }),
            )
            .await,
        );
        let bot_body = body_json(bot_response).await;
        assert_eq!(bot_body["code"], 403, "a bot token must be rejected: {bot_body}");

        scratch.drop_self().await;
    }

    /// `create_object`'s cross-workspace `parent_object_id`/`project_id` check
    /// (`rest-api-v1.md` "`RelationView`"; `ADR-0013` §4): the request fails closed as
    /// `invalid_update` *and* a real `flow_integrity_records` row is written for it — proving the
    /// producer this package was missing, not just the rejection it already had.
    #[tokio::test]
    #[allow(clippy::items_after_statements)]
    async fn cross_workspace_parent_is_rejected_and_recorded_as_an_integrity_alert() {
        let scratch = scratch_or_skip!("cross-workspace-integrity");
        let state = state_for(scratch.db.clone());
        let (workspace_a, owner_a) = seed_workspace(&state, true).await;
        let (workspace_b, owner_b) = seed_workspace(&state, true).await;
        let claims_b = claims_for(owner_b);

        // A real, existing page in workspace A.
        let page_in_a = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims_for(owner_a),
                None,
                Path(workspace_a),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Page In Workspace A".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        let parent_id_in_a = Uuid::parse_str(
            page_in_a["data"]["object"]["id"]
                .as_str()
                .expect("object id is a string"),
        )
        .expect("object id is a uuid");

        // A workspace-B member tries to create a page parented under that workspace-A object.
        let response = to_response(
            create_flow_object(
                State(state.clone()),
                claims_b,
                None,
                Path(workspace_b),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: Some(parent_id_in_a),
                    title: "Cross-Workspace Attempt".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        );
        let body = body_json(response).await;
        assert_eq!(body["code"], 400, "{body}");
        assert_eq!(body["message"], "invalid_update", "{body}");

        use sea_orm::FromQueryResult as _;

        #[derive(sea_orm::FromQueryResult)]
        struct IntegrityRow {
            workspace_id: Uuid,
            kind: String,
            subject_kind: String,
            subject_id: String,
            detected_by: String,
            status: String,
        }
        let rows = IntegrityRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id, kind, subject_kind, subject_id, detected_by, status \
             FROM flow_integrity_records WHERE workspace_id = $1",
            vec![workspace_b.into()],
        ))
        .all(&state.db)
        .await
        .expect("integrity record query runs");

        assert_eq!(
            rows.len(),
            1,
            "exactly one integrity record must be written for the one fail-closed attempt"
        );
        let row = &rows[0];
        assert_eq!(row.workspace_id, workspace_b);
        assert_eq!(row.kind, "cross_workspace_relation");
        assert_eq!(row.subject_kind, "flow_object");
        assert_eq!(row.subject_id, parent_id_in_a.to_string());
        assert_eq!(row.detected_by, "flow.command.create_object");
        assert_eq!(row.status, "open");

        scratch.drop_self().await;
    }

    // ---- Call-direction proofs for `collab_core::limits::check_operation` /
    // `check_operation_batch_count` on the REST content-command path
    // (`flow::command::apply_content_command`), reached through the real
    // `post_flow_object_command` handler these tests drive end to end -- not a unit call into
    // `flow::command` directly, and not the WebSocket path (`flow::collab::write::database_tests`
    // covers that separately via `check_snapshot`).

    /// Runs one command through the real `post_flow_object_command` handler and retries on
    /// envelope `code=409`/`message="server_draining"` until either it stops happening or a
    /// wall-clock deadline passes -- `error-mapping-v1.md`: that code is recoverable, "客户端保留
    /// intent 后重试", the exact behavior a real caller is contractually expected to have, with no
    /// contract-stated upper bound on how long a compliant caller keeps trying. The tests below
    /// submit many real commands/transactions in a tight loop against a real database shared with
    /// the rest of `cargo test --workspace`'s parallel run, so they are exactly the shape most
    /// likely to observe transient lock/rebase contention (`flow::collab::write::database_tests`'s
    /// own `submit` helper documents the same root cause, including sustained multi-second
    /// congestion windows a small fixed attempt count was observed not to outlast). Retrying here
    /// changes nothing about what is under test: `code=400` naming a `limit_kind` (the actual
    /// assertion every caller of this function cares about) is never `server_draining` and is
    /// always returned on the first attempt, unretried; only the recoverable, contract-defined
    /// transient code is retried, and only until `CONTENTION_RETRY_DEADLINE`, so a genuine,
    /// persistent failure still surfaces as a test failure rather than hanging forever. Each retry
    /// uses a fresh `idempotency_key` (the prior attempt was never persisted).
    async fn run_command(
        state: &AppState,
        claims: &Extension<JwtClaims>,
        object_id: Uuid,
        command_type: &str,
        payload: Value,
    ) -> Value {
        const CONTENTION_RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_mins(3);
        const CONTENTION_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(150);
        let started = std::time::Instant::now();
        loop {
            let response = to_response(
                post_flow_object_command(
                    State(state.clone()),
                    claims.clone(),
                    None,
                    Path(object_id),
                    Json(ExecuteFlowCommandRequest {
                        command: FlowCommandEnvelope {
                            command_type: command_type.to_string(),
                            payload: payload.clone(),
                        },
                        expected_frontier: None,
                        idempotency_key: Uuid::new_v4().to_string(),
                        message: None,
                    }),
                )
                .await,
            );
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = body_json(response).await;
            let is_recoverable_contention = body["code"] == 409 && body["message"] == "server_draining";
            if is_recoverable_contention && started.elapsed() < CONTENTION_RETRY_DEADLINE {
                tokio::time::sleep(CONTENTION_RETRY_BACKOFF).await;
                continue;
            }
            return body;
        }
    }

    async fn document_id_for(state: &AppState, object_id: Uuid) -> Uuid {
        #[derive(FromQueryResult)]
        struct Row {
            id: Uuid,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id = $1",
            vec![object_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists")
        .id
    }

    async fn document_head_seq(state: &AppState, document_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            head_seq: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq FROM collab_documents WHERE id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("query runs")
        .expect("row exists")
        .head_seq
    }

    async fn count_event_dispatch(state: &AppState, document_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM event_dispatch WHERE document_id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .n
    }

    async fn count_workspace_business_events(state: &AppState, workspace_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1",
            vec![workspace_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .n
    }

    fn semantic_patch_payload_with_serialized_bytes(target: u64) -> Value {
        let base = json!({
            "operations": [{"op": "set_property", "id": "semantic-block", "key": "fixture", "value": "ok"}],
            "padding": ""
        });
        let base_len = u64::try_from(serde_json::to_vec(&base).expect("serializes").len()).expect("fits");
        let padding = usize::try_from(target - base_len).expect("target fits usize");
        json!({
            "operations": [{"op": "set_property", "id": "semantic-block", "key": "fixture", "value": "ok"}],
            "padding": "x".repeat(padding)
        })
    }

    /// The real REST semantic-patch producer enforces compact JSON bytes before any canonical or
    /// audit write: exact 1 MiB is accepted through the shared CRDT write path, while 1 MiB + 1
    /// returns typed `limit_exceeded(semantic_patch_bytes)` and leaves head/event/dispatch counts
    /// exactly at the accepted boundary.
    #[tokio::test]
    async fn commands_endpoint_semantic_patch_bytes_exact_boundary_accepted_plus_one_rejected_zero_writes() {
        let scratch = scratch_or_skip!("limit-rest-semantic-patch-bytes");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let limit = crate::flow::collab::limits::SEMANTIC_PATCH_JSON_BYTES_MAX;

        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Semantic Patch Bytes Test".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        let object_id =
            Uuid::parse_str(create_body["data"]["object"]["id"].as_str().expect("object id")).expect("object UUID");
        let document_id = document_id_for(&state, object_id).await;
        let insert = run_command(
            &state,
            &claims,
            object_id,
            "insert_block",
            json!({"block_id": "semantic-block"}),
        )
        .await;
        assert_eq!(insert["code"], 0, "{insert}");

        let exact_payload = semantic_patch_payload_with_serialized_bytes(limit);
        assert_eq!(
            u64::try_from(serde_json::to_vec(&exact_payload).expect("serializes").len()).expect("fits"),
            limit
        );
        let exact = run_command(&state, &claims, object_id, "semantic_patch", exact_payload).await;
        assert_eq!(exact["code"], 0, "{exact}");
        let head_after_exact = document_head_seq(&state, document_id).await;
        let events_after_exact = count_workspace_business_events(&state, workspace_id).await;
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;

        let plus_one_payload = semantic_patch_payload_with_serialized_bytes(limit + 1);
        let plus_one = run_command(&state, &claims, object_id, "semantic_patch", plus_one_payload).await;
        assert_eq!(plus_one["code"], 400, "{plus_one}");
        assert_eq!(plus_one["error_code"], "limit_exceeded");
        assert_eq!(plus_one["details"]["limit_kind"], "semantic_patch_bytes");
        assert_eq!(plus_one["details"]["limit"], limit);
        assert_eq!(plus_one["details"]["observed"], limit + 1);
        assert_eq!(document_head_seq(&state, document_id).await, head_after_exact);
        assert_eq!(
            count_workspace_business_events(&state, workspace_id).await,
            events_after_exact,
            "pre-read semantic byte rejection must not even write an audit-only event"
        );
        assert_eq!(count_event_dispatch(&state, document_id).await, dispatch_after_exact);

        scratch.drop_self().await;
    }

    /// The shared workspace-drain producer reaches the real REST handler as HTTP 200 plus the
    /// structured business envelope consumed unchanged by MCP/CLI and mirrored on WS/UI.
    #[tokio::test]
    async fn object_get_surfaces_shared_server_draining_drain_fixture_as_http_200_business_error() {
        let scratch = scratch_or_skip!("rest-server-draining");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Drain Surface Test".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        let object_id =
            Uuid::parse_str(create_body["data"]["object"]["id"].as_str().expect("object id")).expect("object UUID");

        let guard = crate::flow::collab::runtime::runtime().begin_workspace_drain(workspace_id, 2_000);
        let response = to_response(
            get_flow_object(
                State(state.clone()),
                claims,
                None,
                Path(object_id),
                Query(GetFlowObjectQuery {
                    at_seq: None,
                    render: None,
                }),
            )
            .await,
        );
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["code"], 409, "{body}");
        assert_eq!(body["error_code"], "server_draining");
        assert_eq!(body["details"]["reason"], "drain");
        assert_eq!(body["details"]["retry_after_ms"], 2_000);
        drop(guard);

        scratch.drop_self().await;
    }

    /// Call-direction proof for `check_operation`'s `tree_depth` branch: a chain of `insert_block`
    /// commands reaching exactly `tree_depth_max` is accepted one command at a time; the next one
    /// is rejected via body code 400 naming `tree_depth`, and the rejection advances neither the
    /// document head nor `event_dispatch`.
    #[tokio::test]
    async fn commands_endpoint_insert_block_rejects_tree_depth_plus_one_and_accepts_exact_boundary() {
        let scratch = scratch_or_skip!("limit-rest-tree-depth");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let limits = crate::flow::collab::limits::document_limits();

        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Tree Depth Limit Test".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id = Uuid::parse_str(
            create_body["data"]["object"]["id"]
                .as_str()
                .expect("object id is a string"),
        )
        .expect("object id is a uuid");
        let document_id = document_id_for(&state, object_id).await;

        // A chain of `insert_block` commands, each parented on the previous one. The root block
        // (no parent) is depth 0; `tree_depth_max` more commands after it reach exactly
        // `tree_depth_max`, still within the boundary.
        let mut parent_block_id: Option<String> = None;
        let mut accepted_seq = 0i64;
        for i in 0..=limits.tree_depth_max {
            let block_id = format!("depth-{i}");
            let mut payload = json!({ "block_id": block_id });
            if let Some(parent) = &parent_block_id {
                payload["parent_block_id"] = json!(parent);
            }
            let body = run_command(&state, &claims, object_id, "insert_block", payload).await;
            assert_eq!(
                body["code"], 0,
                "creating block at depth {i} (within tree_depth_max={}) must be accepted: {body}",
                limits.tree_depth_max
            );
            accepted_seq = body["data"]["accepted_seq"]
                .as_i64()
                .expect("accepted_seq is an integer");
            parent_block_id = Some(block_id);
        }
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;
        let head_after_exact = document_head_seq(&state, document_id).await;
        assert_eq!(head_after_exact, accepted_seq);

        let one_too_deep = run_command(
            &state,
            &claims,
            object_id,
            "insert_block",
            json!({
                "block_id": "depth-one-too-many",
                "parent_block_id": parent_block_id.expect("the chain above built at least one block"),
            }),
        )
        .await;
        assert_eq!(
            one_too_deep["code"], 400,
            "one block past tree_depth_max must be rejected via body code 400: {one_too_deep}"
        );
        let message = one_too_deep["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("tree_depth"),
            "the rejection message must name limit_kind=tree_depth: {one_too_deep}"
        );
        // The write path this same command also flows through (`write::accept_update`, shared
        // with WebSocket) backstops every per-op structural ceiling with its own
        // `check_snapshot` gate on the fully-merged candidate (`flow::collab::write::
        // database_tests`'s own `ws_structural_limit_tree_depth_*` test covers that gate
        // directly). A REST black-box assertion on `code`/`message` content alone cannot tell
        // "caught early by `apply_content_command`'s `check_operation`" apart from "caught late
        // by that backstop" -- both produce `code=400` naming `tree_depth` -- *unless* it also
        // pins the exact message shape each layer produces: `map_collab_error` (the early,
        // `check_operation` path) renders a bare `"limit_exceeded: tree_depth"`, while
        // `map_write_rejection` (the late, `check_snapshot`-via-`accept_update` path) renders the
        // richer `"limit_exceeded: tree_depth (limit=..., observed=...)"` this same file's
        // `map_write_rejection` builds from `rejected.details`. Asserting the *absence* of that
        // richer shape here is what actually proves this specific command took the early
        // `check_operation` exit and never reached `write::accept_update` at all for this
        // rejection -- not merely that *some* layer, anywhere in the shared write path, rejected.
        assert!(
            !message.contains("observed="),
            "a `parent_block_id` chosen to violate tree_depth must be caught by \
             `apply_content_command`'s own `check_operation` call, before `write::accept_update` \
             is ever reached -- a message carrying '(limit=..., observed=...)' would mean this \
             instead fell through to the shared `check_snapshot` backstop, i.e. that \
             `apply_content_command`'s call site rejected nothing on its own: {one_too_deep}"
        );

        assert_eq!(
            document_head_seq(&state, document_id).await,
            head_after_exact,
            "a rejected command must never advance the document head"
        );
        assert_eq!(
            count_event_dispatch(&state, document_id).await,
            dispatch_after_exact,
            "a rejected command must never produce a new event_dispatch row"
        );

        scratch.drop_self().await;
    }

    fn properties_payload(block_id: &str, count: usize) -> Value {
        let mut properties = serde_json::Map::new();
        for i in 0..count {
            properties.insert(format!("p{i}"), json!(format!("v{i}")));
        }
        json!({ "block_id": block_id, "properties": Value::Object(properties) })
    }

    /// Call-direction proof for `check_operation_batch_count`'s `semantic_patch_operations`
    /// branch -- the one boundary in this handler that `check_operation`'s per-op checks alone
    /// cannot catch (an `update_block` with N `properties` produces N `SetProperty` operations,
    /// and no single one of them, applied in isolation, ever exceeds any per-op structural
    /// ceiling -- only the *batch count* does), and the one case where the WebSocket-shared
    /// `check_snapshot` backstop in `hydrate_and_apply` genuinely cannot substitute for this
    /// REST-path-only check: a final document with 101 properties on one block violates no
    /// `check_snapshot` aggregate at all. `properties` at exactly `semantic_patch_operations_max`
    /// is accepted in one call; one more is rejected, with zero effect on the document head or
    /// `event_dispatch`.
    #[tokio::test]
    async fn commands_endpoint_update_block_rejects_semantic_patch_operations_batch_plus_one_and_accepts_exact_boundary()
     {
        let scratch = scratch_or_skip!("limit-rest-batch-count");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let limits = crate::flow::collab::limits::document_limits();

        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Batch Count Limit Test".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id = Uuid::parse_str(
            create_body["data"]["object"]["id"]
                .as_str()
                .expect("object id is a string"),
        )
        .expect("object id is a uuid");
        let document_id = document_id_for(&state, object_id).await;

        let insert_body = run_command(
            &state,
            &claims,
            object_id,
            "insert_block",
            json!({ "block_id": "batch-target" }),
        )
        .await;
        assert_eq!(insert_body["code"], 0, "{insert_body}");

        let exact_count = limits.semantic_patch_operations_max;
        let exact_body = run_command(
            &state,
            &claims,
            object_id,
            "update_block",
            properties_payload("batch-target", exact_count),
        )
        .await;
        assert_eq!(
            exact_body["code"], 0,
            "exactly semantic_patch_operations_max properties in one update_block call must be accepted: {exact_body}"
        );
        let accepted_seq = exact_body["data"]["accepted_seq"]
            .as_i64()
            .expect("accepted_seq is an integer");
        let dispatch_after_exact = count_event_dispatch(&state, document_id).await;
        let head_after_exact = document_head_seq(&state, document_id).await;
        assert_eq!(head_after_exact, accepted_seq);

        let plus_one_body = run_command(
            &state,
            &claims,
            object_id,
            "update_block",
            properties_payload("batch-target", exact_count + 1),
        )
        .await;
        assert_eq!(
            plus_one_body["code"], 400,
            "one property past semantic_patch_operations_max must be rejected via body code 400: {plus_one_body}"
        );
        // The structured envelope, not the message text: `error-mapping-v1.md` requires REST to
        // carry `error_code` plus `details={limit_kind,limit,observed?}` so a caller branches on
        // fields rather than substring-matching prose. Asserting only `message.contains(...)`
        // (what this test did before) would still pass if `details` were dropped entirely.
        assert_eq!(plus_one_body["error_code"], "limit_exceeded", "{plus_one_body}");
        let details = &plus_one_body["details"];
        assert_eq!(
            details["limit_kind"], "semantic_patch_operations",
            "the rejection must name limit_kind=semantic_patch_operations in structured details: {plus_one_body}"
        );
        assert_eq!(details["limit"], exact_count as u64, "{plus_one_body}");
        assert_eq!(details["observed"], (exact_count + 1) as u64, "{plus_one_body}");

        assert_eq!(
            document_head_seq(&state, document_id).await,
            head_after_exact,
            "a batch-count-rejected command must never advance the document head"
        );
        assert_eq!(
            count_event_dispatch(&state, document_id).await,
            dispatch_after_exact,
            "a batch-count-rejected command must never produce a new event_dispatch row \
             -- this is the atomicity proof: none of the 101 properties in the rejected call were \
             ever applied, not even the first 100 that would individually have been fine"
        );

        scratch.drop_self().await;
    }

    /// `page_size` (`page_limit_max=100`): `list_flow_objects` -> `query::list_objects` ->
    /// `query::validate_limit`. `limit=100` is accepted; `limit=101` is rejected through the
    /// same `ApiError::limit_exceeded` typed path every other `limit_kind` uses, so the REST
    /// envelope actually carries `error_code="limit_exceeded"` and
    /// `details={limit_kind,limit,observed}` (`error.rs`'s `ApiResponse`-backed `Typed` arm),
    /// not a bare message string. No document/`event_dispatch` side effect to check here: this
    /// is a read-only list endpoint, not a write path.
    #[tokio::test]
    async fn list_objects_endpoint_rejects_page_size_over_page_limit_max_and_accepts_exact_boundary() {
        const PAGE_LIMIT_MAX: u64 = 100;

        let scratch = scratch_or_skip!("page-size-boundary");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let list_query = |limit: Option<u64>| ListFlowObjectsQuery {
            project_id: None,
            unprojected: false,
            object_type: None,
            parent_id: None,
            q: None,
            cursor: None,
            limit,
            include_archived: false,
        };

        // ---- exact boundary: limit=page_limit_max is accepted ----
        let exact_response = to_response(
            list_flow_objects(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Query(list_query(Some(PAGE_LIMIT_MAX))),
            )
            .await,
        );
        assert_eq!(exact_response.status(), axum::http::StatusCode::OK);
        let exact_body = body_json(exact_response).await;
        assert_eq!(
            exact_body["code"], 0,
            "limit=page_limit_max must be accepted: {exact_body}"
        );

        // ---- plus one: limit=page_limit_max+1 is rejected limit_kind=page_size ----
        let plus_one_response = to_response(
            list_flow_objects(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Query(list_query(Some(PAGE_LIMIT_MAX + 1))),
            )
            .await,
        );
        assert_eq!(
            plus_one_response.status(),
            axum::http::StatusCode::OK,
            "REST is always a 200 envelope; business failure is in the body's `code`"
        );
        let plus_one_body = body_json(plus_one_response).await;
        assert_ne!(
            plus_one_body["code"], 0,
            "limit=page_limit_max+1 must be rejected: {plus_one_body}"
        );
        assert_eq!(plus_one_body["error_code"], "limit_exceeded");
        assert_eq!(plus_one_body["details"]["limit_kind"], "page_size");
        assert_eq!(plus_one_body["details"]["limit"], PAGE_LIMIT_MAX);
        assert_eq!(plus_one_body["details"]["observed"], PAGE_LIMIT_MAX + 1);

        scratch.drop_self().await;
    }

    /// `flow.command.rejected` is written for a bot's rejected command as well as a user's.
    ///
    /// This producer is the one that could fail **silently**: `record_command_rejected` logs its
    /// own insert failure with `tracing::error!` and returns, by design, so that a failed audit
    /// write never turns a correctly-rejected command into a 500. The cost of that design is that
    /// it must never be handed a row the database will refuse — and it was: it wrote
    /// `actor_id: Some(actor_id)` into a `users(id)` FK, so every bot-triggered rejection violated
    /// the constraint, was logged, and **vanished**. The command still returned its correct 409,
    /// which is exactly why nothing noticed: the only observable difference was an audit row that
    /// was never there.
    ///
    /// Rejecting the same command for a user and for a bot must therefore leave *two* rows.
    #[tokio::test]
    async fn a_rejected_command_is_audited_for_a_bot_exactly_as_it_is_for_a_user() {
        #[derive(FromQueryResult)]
        struct RejectedRow {
            actor_id: Option<Uuid>,
            source: Value,
        }

        let scratch = scratch_or_skip!("rejected-audit-bot");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let bot_id = Uuid::new_v4();
        let bot = Extension(crate::middleware::bot_auth::BotAuthContext {
            bot_id,
            workspace_id,
            permissions: vec!["read".to_string(), "write".to_string(), "admin".to_string()],
            surface: crate::flow::event_origin::EventSurface::McpStdio,
            tool_name: Some("flow.object_command".to_string()),
            request_id: Uuid::new_v4(),
        });

        // One object per caller, so each caller's second `archive` is the rejected one.
        let make_object = |title: &str| {
            let state = state.clone();
            let claims = claims.clone();
            let title = title.to_string();
            async move {
                let body = body_json(to_response(
                    create_flow_object(
                        State(state),
                        claims,
                        None,
                        Path(workspace_id),
                        Json(CreateFlowObjectRequest {
                            object_type: "page".to_string(),
                            project_id: None,
                            parent_object_id: None,
                            title,
                            idempotency_key: Uuid::new_v4().to_string(),
                            message: None,
                            initial_fields: Vec::new(),
                            initial_view: None,
                        }),
                    )
                    .await,
                ))
                .await;
                Uuid::parse_str(body["data"]["object"]["id"].as_str().expect("id")).expect("uuid")
            }
        };
        let user_object = make_object("User Archive").await;
        let bot_object = make_object("Bot Archive").await;

        let archive = |object_id: Uuid, as_bot: Option<Extension<crate::middleware::bot_auth::BotAuthContext>>| {
            let state = state.clone();
            let claims = claims.clone();
            async move {
                body_json(to_response(
                    post_flow_object_command(
                        State(state),
                        claims,
                        as_bot,
                        Path(object_id),
                        Json(ExecuteFlowCommandRequest {
                            command: FlowCommandEnvelope {
                                command_type: "archive".to_string(),
                                payload: json!({}),
                            },
                            expected_frontier: None,
                            idempotency_key: Uuid::new_v4().to_string(),
                            message: None,
                        }),
                    )
                    .await,
                ))
                .await
            }
        };

        // First archive succeeds, second is rejected — for each caller kind.
        assert_eq!(archive(user_object, None).await["code"], 0);
        let user_rejected = archive(user_object, None).await;
        assert_ne!(
            user_rejected["code"], 0,
            "archiving twice must be rejected: {user_rejected}"
        );

        assert_eq!(archive(bot_object, Some(bot.clone())).await["code"], 0);
        let bot_rejected = archive(bot_object, Some(bot)).await;
        assert_ne!(
            bot_rejected["code"], 0,
            "archiving twice must be rejected: {bot_rejected}"
        );

        let rows = RejectedRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT actor_id, source FROM business_events WHERE workspace_id = $1 \
              AND event_type = 'flow.command.rejected' ORDER BY created_at, id",
            vec![workspace_id.into()],
        ))
        .all(&state.db)
        .await
        .expect("business_events query runs");
        assert_eq!(
            rows.len(),
            2,
            "both the user's and the bot's rejection must be audited; a bot rejection that leaves no row \
             is the audit stream losing an event without anyone being told"
        );

        let user_row = rows
            .iter()
            .find(|row| row.source["surface"] == "rest")
            .expect("the user's rejection was recorded");
        assert_eq!(
            user_row.actor_id,
            Some(owner_id),
            "a user's rejection still names the user"
        );
        let bot_row = rows
            .iter()
            .find(|row| row.source["surface"] == "mcp_stdio")
            .expect("the bot's rejection was recorded");
        assert_eq!(
            bot_row.actor_id, None,
            "a bot's rejection carries no `users(id)` actor — that is what made it insertable at all"
        );

        scratch.drop_self().await;
    }

    /// The bot behind an event, recovered the only way it can be — through the **real
    /// middleware**, over HTTP, with a real bot token.
    ///
    /// `business_events.actor_id` is `NULL` for a bot (it is a `users(id)` FK and a bot id is not
    /// a user id), so the claim that bot attribution survives rests entirely on one join:
    ///
    /// ```text
    /// business_events.source->>'request'  ==  bot_operation_logs.request_id  ->  bot_id
    /// ```
    ///
    /// That join exists only because `middleware::bot_auth::bot_auth_context` mints **one**
    /// `request_id` per request and both the audit event and the operation log copy that same
    /// value. Nothing had ever executed it: the existing assertions only checked that
    /// `source.request` parses as a UUID, which is true of any UUID at all — including a fresh
    /// one that joins to nothing. Replacing `bot.request_id` with `Uuid::new_v4()` in
    /// [`request_origin`] left the whole suite green while silently severing bot attribution.
    ///
    /// This test runs the production `bot_or_user_auth_middleware` against a real
    /// `workspace_bots` row, so the header → middleware → `BotAuthContext` → envelope chain is
    /// executed rather than simulated by constructing the context in Rust.
    // The axum route pattern below contains `{workspace_id}`, which is axum's path-parameter
    // syntax and not a format argument, but is indistinguishable from one to the lint.
    #[allow(clippy::literal_string_with_formatting_args)]
    #[tokio::test]
    async fn the_bot_behind_an_event_is_recoverable_through_the_request_id_the_middleware_minted() {
        #[derive(FromQueryResult)]
        struct EventRow {
            actor_id: Option<Uuid>,
            source: Value,
        }
        #[derive(FromQueryResult)]
        struct JoinedBot {
            bot_id: Uuid,
            tool_name: Option<String>,
            surface: String,
        }

        let scratch = scratch_or_skip!("bot-attribution-join");
        let state = state_for(scratch.db.clone());
        let (workspace_id, _owner_id) = seed_workspace(&state, true).await;

        // A real bot token, hashed exactly the way the middleware hashes it.
        let bot_id = Uuid::new_v4();
        let raw_token = format!("opr_{}", Uuid::new_v4().simple());
        let token_hash = {
            use sha2::{Digest as _, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(raw_token.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        exec(
            &state,
            "INSERT INTO workspace_bots \
             (id, workspace_id, name, token_hash, token_prefix, permissions, transport_surface, is_active) \
             VALUES ($1, $2, 'attribution-bot', $3, $4, '[\"read\",\"write\",\"admin\"]'::jsonb, \
             'mcp_stdio', true)",
            vec![
                bot_id.into(),
                workspace_id.into(),
                token_hash.into(),
                raw_token[..8].to_string().into(),
            ],
        )
        .await;

        // The create route behind the **production** auth middleware.
        let app = axum::Router::new()
            .route(
                "/api/v1/flow/workspaces/{workspace_id}/objects",
                axum::routing::post(create_flow_object),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::bot_auth::bot_or_user_auth_middleware,
            ))
            .with_state(state.clone());

        let response = {
            use tower::ServiceExt as _;
            app.oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri(format!("/api/v1/flow/workspaces/{workspace_id}/objects"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {raw_token}"))
                    .header("x-openpr-mcp-surface", "mcp_stdio")
                    .header("x-openpr-mcp-tool", "flow.object_create")
                    .body(axum::body::Body::from(
                        json!({
                            "object_type": "page",
                            "title": "Attributable",
                            "idempotency_key": Uuid::new_v4().to_string(),
                        })
                        .to_string(),
                    ))
                    .expect("the request builds"),
            )
            .await
            .expect("the router responds")
        };
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["code"], 0,
            "a real bot token over HTTP must be able to create: {body}"
        );

        // The event: no actor (the FK forbids it), but the transport the middleware resolved.
        let event = EventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT actor_id, source FROM business_events WHERE workspace_id = $1 \
              AND event_type = 'flow.object.created'",
            vec![workspace_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("business_events query runs")
        .expect("the create wrote its event");
        assert_eq!(event.actor_id, None, "a bot's actor_id must be NULL, not a bot id");
        assert_eq!(
            event.source["surface"], "mcp_stdio",
            "the surface must come from the header the real middleware parsed: {:?}",
            event.source
        );
        assert_eq!(
            event.source["attestation"], "attested",
            "a real bot request whose transport matches its credential must be attested"
        );
        assert_eq!(
            event.source["tool"], "flow.object_create",
            "the exact registered tool must reach the envelope: {:?}",
            event.source
        );
        let request_id = event.source["request"].as_str().expect("source.request is a string");

        // `spawn_operation_log` is `tokio::spawn`ed, so give it a bounded moment to land.
        let mut joined = None;
        for _ in 0..40 {
            joined = JoinedBot::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT bot_id, tool_name, surface FROM bot_operation_logs WHERE request_id = $1::uuid",
                vec![request_id.into()],
            ))
            .one(&state.db)
            .await
            .expect("bot_operation_logs query runs");
            if joined.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // **The whole point.** Not "the request id parses" — the join resolves, and it resolves to
        // *this* bot.
        let joined = joined.expect(
            "`business_events.source->>'request'` must join to a `bot_operation_logs` row: that join is the \
             only thing that names the bot behind an event whose `actor_id` is NULL",
        );
        assert_eq!(
            joined.bot_id, bot_id,
            "the join must recover the bot that actually made the call"
        );
        assert_eq!(
            joined.surface, "mcp_stdio",
            "both sides must record one resolved transport"
        );
        assert_eq!(joined.tool_name.as_deref(), Some("flow.object_create"));

        scratch.drop_self().await;
    }

    /// Every bot-reachable Flow write route, exercised **by a bot**, because until now none of
    /// them worked.
    ///
    /// Measured, not inferred (the earlier report said "大概率 500" and declined to claim it):
    /// with a bot token, `POST .../objects` returned `500 database error`
    /// (`flow_objects_created_by_fkey`), `PUT .../features/flow` returned `500 database error`
    /// (`flow_workspace_settings_updated_by_fkey`), and `POST .../commands` returned
    /// `409 server_draining/contention` — the last one worst of all, because
    /// `business_events_actor_id_fkey` aborted the locked phase, the write path retried it
    /// `MAX_REBASE_ATTEMPTS` times and then reported a **retryable** rejection for a write that
    /// could never succeed. `PUT .../grants` was the only one that worked, because
    /// `flow::grants` was the only module that had ever handled the case.
    ///
    /// The cause is one mismatch: `middleware::bot_auth` returns the **bot id** as the actor, and
    /// every "who did this" column on these paths is `REFERENCES users(id)`. See
    /// `flow::command::actor_user_id`.
    #[tokio::test]
    async fn every_bot_reachable_write_route_works_for_a_bot_and_stays_attributable() {
        #[derive(FromQueryResult)]
        struct ActorRow {
            event_type: String,
            actor_id: Option<Uuid>,
            source: Value,
        }
        #[derive(FromQueryResult)]
        struct UpdateActor {
            actor_id: Option<Uuid>,
        }

        let scratch = scratch_or_skip!("bot-write-routes");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let bot_id = Uuid::new_v4();
        let bot = |tool: &str| {
            Extension(crate::middleware::bot_auth::BotAuthContext {
                bot_id,
                workspace_id,
                permissions: vec!["read".to_string(), "write".to_string(), "admin".to_string()],
                surface: crate::flow::event_origin::EventSurface::McpStdio,
                tool_name: Some(tool.to_string()),
                request_id: Uuid::new_v4(),
            })
        };

        // ---- create, as a bot ----
        let created = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                Some(bot("flow.object_create")),
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Bot Created".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            created["code"], 0,
            "a bot must be able to create a Flow object: {created}"
        );
        let object_id = Uuid::parse_str(created["data"]["object"]["id"].as_str().expect("id")).expect("uuid");

        // ---- a content command, as a bot ----
        let renamed = body_json(to_response(
            post_flow_object_command(
                State(state.clone()),
                claims.clone(),
                Some(bot("flow.object_command")),
                Path(object_id),
                Json(ExecuteFlowCommandRequest {
                    command: FlowCommandEnvelope {
                        command_type: "set_title".to_string(),
                        payload: json!({"title": "Bot Renamed"}),
                    },
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            renamed["code"], 0,
            "a bot content command must not be reported as retryable contention: {renamed}"
        );

        // ---- the feature flag, as a bot: the live `flow.feature_set` MCP tool ----
        // Flipped to `false` so it is a real transition and really writes its event; done last,
        // because the routes above need Flow enabled.
        let feature = body_json(to_response(
            set_flow_feature(
                State(state.clone()),
                claims.clone(),
                Some(bot("flow.feature_set")),
                Path(workspace_id),
                Json(SetFlowFeatureRequest {
                    enabled: Some(false),
                    default_member_level: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(
            feature["code"], 0,
            "a bot must be able to set the Flow feature flag: {feature}"
        );
        assert!(
            feature["data"]["event_id"].is_string(),
            "flipping the flag is a real transition and must record one: {feature}"
        );

        // ---- what landed ----
        let rows = ActorRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT event_type, actor_id, source FROM business_events WHERE workspace_id = $1 \
             ORDER BY created_at, id",
            vec![workspace_id.into()],
        ))
        .all(&state.db)
        .await
        .expect("business_events query runs");
        for expected in ["flow.object.created", "flow.content.accepted", "flow.feature.disabled"] {
            assert!(
                rows.iter().any(|row| row.event_type == expected),
                "'{expected}' must have been written by a bot; got {:?}",
                rows.iter().map(|row| row.event_type.as_str()).collect::<Vec<_>>()
            );
        }
        for row in &rows {
            assert_eq!(
                row.actor_id, None,
                "'{}' was written by a bot, so `actor_id` — a `users(id)` FK — must be NULL rather \
                 than a bot id that no `users` row matches",
                row.event_type
            );
            // Attribution is not lost by that NULL: `source.request` is the very `request_id` the
            // middleware wrote to `bot_operation_logs.request_id`, so the bot behind any of these
            // events is one join away. That only holds because the two are deliberately the same
            // value (`middleware::bot_auth::bot_auth_context`).
            assert_eq!(row.source["surface"], "mcp_stdio", "{:?}", row.source);
            assert!(
                row.source["request"]
                    .as_str()
                    .is_some_and(|r| Uuid::parse_str(r).is_ok()),
                "'{}' must carry the middleware's request id, which is what makes the bot \
                 recoverable from `bot_operation_logs`: {:?}",
                row.event_type,
                row.source
            );
        }

        // The content write's own `collab_updates` row has the same `users(id)` FK.
        let updates = UpdateActor::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT actor_id FROM collab_updates WHERE document_id = $1",
            vec![document_id_for(&state, object_id).await.into()],
        ))
        .all(&state.db)
        .await
        .expect("collab_updates query runs");
        assert!(
            !updates.is_empty(),
            "the bot's content command must have persisted an update"
        );
        for update in &updates {
            assert_eq!(
                update.actor_id, None,
                "`collab_updates.actor_id` is a `users(id)` FK too"
            );
        }

        scratch.drop_self().await;
    }

    /// 判据 (a) and (b) of `events-v1.md`'s 2026-09-01 clause, through the **real route seam**.
    ///
    /// > (a) 同一条 route 被不同 transport 打到时，落库的 `source.surface` **必须不同**；
    /// > (b) `source.request` 必须**每请求不同**（同一请求的多条事件相同）。
    ///
    /// The first version of this work package moved the hardcoded surface from the producers into
    /// the route layer and stopped — so `PUT .../grants` filled a constant `rest` no matter who
    /// called it, and the only tests that exercised a non-REST surface constructed a
    /// `CommandOrigin` by hand in the domain layer, which cannot observe a handler that ignores
    /// its own auth context. This test drives the *same route* four times over four transports and
    /// reads what landed in `business_events`, so a handler that unconditionally answers `rest`
    /// fails it.
    ///
    /// It also pins (b) in the form that can actually fail: the REST call writes **two** events in
    /// one request, so "same within one request" and "different across requests" are both
    /// observable. Asserting only that `source.request` is a string — which is what the earlier
    /// version did — stays green for any constant whatsoever.
    #[tokio::test]
    async fn the_same_route_records_the_transport_it_was_reached_over_and_one_request_id_per_request() {
        #[derive(FromQueryResult)]
        struct PermissionRow {
            id: Uuid,
            event_type: String,
            source: Value,
            causation_id: Option<Uuid>,
            idempotency_key: Option<String>,
            payload: Value,
        }

        let scratch = scratch_or_skip!("route-transport-origin");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);
        let bot_id = Uuid::new_v4();

        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "Transport Origin".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id =
            Uuid::parse_str(create_body["data"]["object"]["id"].as_str().expect("object id")).expect("object UUID");

        let bot_extension = |surface: crate::flow::event_origin::EventSurface, tool: &str| {
            Extension(crate::middleware::bot_auth::BotAuthContext {
                bot_id,
                workspace_id,
                permissions: vec!["read".to_string(), "write".to_string(), "admin".to_string()],
                surface,
                tool_name: Some(tool.to_string()),
                // The middleware mints this once per request; a fresh one here is what a fresh
                // request looks like.
                request_id: Uuid::new_v4(),
            })
        };
        let grant = |kind: &str, id: Uuid, level: &str| GrantRequestBody {
            principal_kind: kind.to_string(),
            principal_id: id,
            level: level.to_string(),
        };
        let put_grants = |bot: Option<Extension<crate::middleware::bot_auth::BotAuthContext>>,
                          grants: Vec<GrantRequestBody>| {
            let state = state.clone();
            let claims = claims.clone();
            async move {
                body_json(to_response(
                    put_flow_object_grants(
                        State(state),
                        claims,
                        bot,
                        Path(object_id),
                        Json(SetGrantsRequest {
                            grants,
                            confirm_self_lockout: true,
                            dry_run: false,
                            idempotency_key: Uuid::new_v4().to_string(),
                        }),
                    )
                    .await,
                ))
                .await
            }
        };

        // ---- leg 1: REST (JWT direct), writing two permission events in one request ----
        // The bot is granted `full_access` here so the three MCP legs below can act at all: a bot
        // token does *not* inherit the workspace-admin bypass (`flow::collab::authz` grants that
        // only to `principal_kind == "user"`), so it needs an explicit object grant.
        let rest_body = put_grants(
            None,
            vec![
                grant("user", owner_id, "full_access"),
                grant("bot", bot_id, "full_access"),
            ],
        )
        .await;
        assert_eq!(rest_body["code"], 0, "{rest_body}");

        // ---- legs 2-4: the same route over each MCP transport ----
        // Each leg flips the owner's own level so it really changes a row and really writes an
        // event; the bot keeps `full_access` so it can still act on the next leg.
        for (surface, level) in [
            (crate::flow::event_origin::EventSurface::McpHttp, "edit"),
            (crate::flow::event_origin::EventSurface::McpSse, "full_access"),
            (crate::flow::event_origin::EventSurface::McpStdio, "edit"),
        ] {
            let body = put_grants(
                Some(bot_extension(surface, "objects.grants_set")),
                vec![grant("user", owner_id, level), grant("bot", bot_id, "full_access")],
            )
            .await;
            assert_eq!(body["code"], 0, "{} leg: {body}", surface.as_wire());
        }

        let rows = PermissionRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, event_type, source, causation_id, idempotency_key, payload FROM business_events \
             WHERE workspace_id = $1 AND event_type LIKE 'flow.permission.%' ORDER BY created_at, id",
            vec![workspace_id.into()],
        ))
        .all(&state.db)
        .await
        .expect("business_events query runs");
        assert!(
            rows.len() >= 5,
            "expected the REST leg's two events plus one per MCP leg, got {}",
            rows.len()
        );

        // ---- (a) one route, four transports, four surfaces ----
        let surfaces: std::collections::BTreeSet<&str> =
            rows.iter().filter_map(|row| row.source["surface"].as_str()).collect();
        assert_eq!(
            surfaces,
            ["mcp_http", "mcp_sse", "mcp_stdio", "rest"].into_iter().collect(),
            "the same route must record the transport it was reached over, not a constant"
        );
        for row in &rows {
            let surface = row.source["surface"].as_str().unwrap_or_default();
            assert_eq!(row.source["attestation"], "attested");
            if surface == "rest" {
                assert!(
                    row.source.get("tool").is_none(),
                    "a JWT-direct REST call has no tool concept, so the key must be omitted: {:?}",
                    row.source
                );
            } else {
                assert_eq!(
                    row.source["tool"], "objects.grants_set",
                    "an MCP call must carry the exact registered tool the middleware resolved: {:?}",
                    row.source
                );
            }
        }

        // ---- (b) one request id per request, shared by every event of that request ----
        let rest_requests: std::collections::BTreeSet<&str> = rows
            .iter()
            .filter(|row| row.source["surface"] == "rest")
            .filter_map(|row| row.source["request"].as_str())
            .collect();
        assert_eq!(
            rest_requests.len(),
            1,
            "the REST leg wrote several events in one request, so they must share one request id, got {rest_requests:?}"
        );
        let all_requests: Vec<&str> = rows.iter().filter_map(|row| row.source["request"].as_str()).collect();
        let distinct: std::collections::BTreeSet<&str> = all_requests.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            4,
            "four requests must mint four request ids (one shared within the REST leg), got {distinct:?}"
        );
        for request in &distinct {
            assert!(
                Uuid::parse_str(request).is_ok(),
                "`source.request` must be the server's own request id, got {request:?}"
            );
        }

        // ---- the primary event is the one carrying the caller's key, and the rest name it ----
        // `events-v1.md` (2026-09-01 订正): "主事件 = 携带调用方 `idempotency_key` 的那一条".
        let rest_leg: Vec<&PermissionRow> = rows.iter().filter(|row| row.source["surface"] == "rest").collect();
        let keyed: Vec<&&PermissionRow> = rest_leg.iter().filter(|row| row.idempotency_key.is_some()).collect();
        assert_eq!(
            keyed.len(),
            1,
            "exactly one event of a command may carry the caller's key — that is what makes it the primary"
        );
        let primary = keyed[0];
        assert_eq!(
            primary.causation_id, None,
            "the primary event of a first user request roots the chain"
        );
        assert_eq!(
            primary.payload["principal_kind"], "bot",
            "the primary is chosen from the events' own content (`bot` sorts before `user`), not from \
             whichever principal the iteration happened to reach first"
        );
        for row in rest_leg.iter().filter(|row| row.id != primary.id) {
            assert_eq!(
                row.causation_id,
                Some(primary.id),
                "'{}' must name the command's primary event as its causation",
                row.event_type
            );
        }

        scratch.drop_self().await;
    }

    /// The REST surface **declaring** its own origin, end to end through the real handlers.
    ///
    /// `events-v1.md`: "`source` 由服务端按 Web/REST/MCP/CLI/worker 覆盖". Every event this
    /// request writes must carry `surface="rest"` *because `routes::flow::request_origin` resolved
    /// a JWT-direct call to REST*,
    /// not because a producer hardcoded it — the producer-side half of that statement is proven
    /// by `flow::move_object`'s and `flow::grants`' non-REST origin tests, which run the same
    /// producers from `mcp_stdio`/`mcp_http`/`cli_tools_call` and get those surfaces back.
    ///
    /// This also pins a defect the split fixed rather than merely restructured: a REST content
    /// command reached `write::stage_locked_writes`, which stamped the literal `"web"` on the
    /// `flow.content.accepted` envelope **and** on `collab_updates.origin_surface`. Every
    /// `set_title` issued over REST was recorded as a WebSocket write.
    #[tokio::test]
    async fn every_event_a_rest_request_writes_carries_the_rest_surface_and_a_server_request_id() {
        #[derive(FromQueryResult)]
        struct Row {
            event_type: String,
            source: Value,
            correlation_id: Option<Uuid>,
            /// Selected because an unselected column cannot be asserted on, and this one guards a
            /// real regression: `flow.command.rejected` once filled `causation_id` with a fresh
            /// `Uuid::new_v4()`, a dangling edge pointing at an event that never existed. Nothing
            /// caught it, because this row type did not read the column.
            causation_id: Option<Uuid>,
        }
        #[derive(FromQueryResult)]
        struct SurfaceRow {
            origin_surface: String,
        }

        let scratch = scratch_or_skip!("rest-origin-surface");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let claims = claims_for(owner_id);

        let create_body = body_json(to_response(
            create_flow_object(
                State(state.clone()),
                claims.clone(),
                None,
                Path(workspace_id),
                Json(CreateFlowObjectRequest {
                    object_type: "page".to_string(),
                    project_id: None,
                    parent_object_id: None,
                    title: "REST Origin Test".to_string(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: Vec::new(),
                    initial_view: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(create_body["code"], 0, "{create_body}");
        let object_id =
            Uuid::parse_str(create_body["data"]["object"]["id"].as_str().expect("object id")).expect("object UUID");
        let document_id = document_id_for(&state, object_id).await;

        let renamed = run_command(&state, &claims, object_id, "set_title", json!({"title": "Renamed"})).await;
        assert_eq!(renamed["code"], 0, "{renamed}");

        let rows = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT event_type, source, correlation_id, causation_id FROM business_events \
             WHERE workspace_id = $1 ORDER BY created_at, id",
            vec![workspace_id.into()],
        ))
        .all(&state.db)
        .await
        .expect("business_events query runs");

        let types: Vec<&str> = rows.iter().map(|row| row.event_type.as_str()).collect();
        assert!(
            types.contains(&"flow.object.created") && types.contains(&"flow.content.accepted"),
            "expected the create and the content command to be recorded, got {types:?}"
        );
        for row in &rows {
            assert_eq!(
                row.source["surface"], "rest",
                "'{}' must carry the surface the REST entry point declared, got {:?}",
                row.event_type, row.source
            );
            assert_eq!(
                row.causation_id, None,
                "'{}' was written by a first user request, so its causation must be NULL — not a \
                 freshly minted id pointing at an event that never existed",
                row.event_type
            );
            assert!(
                row.source["request"].is_string(),
                "'{}' must carry the server-generated request id `request_origin` fills, got {:?}",
                row.event_type,
                row.source
            );
            for absent in ["session", "tool", "client_id", "service"] {
                assert!(
                    row.source.get(absent).is_none(),
                    "REST has no {absent}; `events-v1.md` says an inapplicable key is omitted, but \
                     '{}' carried {:?}",
                    row.event_type,
                    row.source
                );
            }
            assert!(
                row.correlation_id.is_some(),
                "'{}' must carry the correlation its request generated",
                row.event_type
            );
        }

        // Two separate HTTP requests are two separate causal chains. Stated as "the create's
        // correlation is not the content command's" rather than as an exact count: `run_command`
        // retries a `server_draining` rejection as a fresh request, and each retry legitimately
        // roots its own correlation (and writes its own `flow.command.rejected`), so a count
        // would be asserting on contention rather than on the contract.
        let created_correlation = rows
            .iter()
            .find(|row| row.event_type == "flow.object.created")
            .and_then(|row| row.correlation_id)
            .expect("the create wrote a correlation");
        let accepted_correlation = rows
            .iter()
            .find(|row| row.event_type == "flow.content.accepted")
            .and_then(|row| row.correlation_id)
            .expect("the content command wrote a correlation");
        assert_ne!(
            created_correlation, accepted_correlation,
            "two separate HTTP requests must root two separate causal chains"
        );

        // The column the WebSocket literal used to poison, read straight out of the row the
        // content command wrote.
        let surfaces: Vec<String> = SurfaceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT origin_surface FROM collab_updates WHERE document_id = $1 ORDER BY seq",
            vec![document_id.into()],
        ))
        .all(&state.db)
        .await
        .expect("collab_updates query runs")
        .into_iter()
        .map(|row| row.origin_surface)
        .collect();
        assert_eq!(
            surfaces,
            vec!["rest".to_string()],
            "a REST content command must be recorded as a REST write, not a WebSocket one"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn relations_handler_returns_a_real_link_through_the_rest_shape() {
        let scratch = scratch_or_skip!("relations-handler");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let source = create_page_as_owner(&state, workspace_id, owner_id, "Source").await;
        let target = create_page_as_owner(&state, workspace_id, owner_id, "Target").await;

        let linked = body_json(to_response(
            post_flow_object_command(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source),
                Json(ExecuteFlowCommandRequest {
                    command: FlowCommandEnvelope {
                        command_type: "link".to_string(),
                        payload: json!({
                            "target_object_id": target,
                            "relation_type": "related_to",
                            "properties": {"label": "visible"},
                        }),
                    },
                    expected_frontier: None,
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(linked["code"], 0, "{linked}");

        let page = body_json(to_response(
            get_flow_object_relations(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(source),
                Query(FlowRelationsQuery {
                    direction: Some("outgoing".to_string()),
                    relation_type: Some("related_to".to_string()),
                    cursor: None,
                    limit: Some(50),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(page["code"], 0, "{page}");
        assert_eq!(page["data"]["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(page["data"]["items"][0]["visibility"], "visible");
        assert_eq!(page["data"]["items"][0]["other_object"]["id"], target.to_string());
        assert_eq!(page["data"]["items"][0]["relation_type"], "related_to");
        assert!(page["data"].get("total").is_none());
        assert!(page["data"].get("filtered_count").is_none());
        assert!(page["data"].get("examined").is_none());

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn delivery_replay_route_requires_admin_and_replays_identical_idempotency_key() {
        let scratch = scratch_or_skip!("delivery-replay-route");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let now = chrono::Utc::now();
        let request = ReplayDeliveriesRequest {
            mode: crate::events::dispatcher::ReplayMode::Rebuild,
            event_type: None,
            subscriber_kind: Some("webhook".to_string()),
            subscriber_id: None,
            from: now - chrono::Duration::hours(2),
            to: now - chrono::Duration::hours(1),
            dry_run: true,
            confirm: true,
            idempotency_key: "replay-route-key".to_string(),
        };
        let denied = post_flow_delivery_replay(
            State(state.clone()),
            claims_for(member_id),
            None,
            Path(workspace_id),
            Json(request.clone()),
        )
        .await;
        assert!(denied.is_err(), "a non-admin member must be rejected");

        let first = body_json(to_response(
            post_flow_delivery_replay(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(request.clone()),
            )
            .await,
        ))
        .await;
        let second = body_json(to_response(
            post_flow_delivery_replay(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(request.clone()),
            )
            .await,
        ))
        .await;
        assert_eq!(first["code"], 0, "{first}");
        assert_eq!(
            first["data"], second["data"],
            "identical replay must return the stored result"
        );
        let ledger_rows = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_replay_requests WHERE workspace_id=$1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("ledger count runs")
            .expect("ledger count row")
            .try_get::<i64>("", "n")
            .expect("ledger count reads");
        assert_eq!(ledger_rows, 1);

        let mut drift = request;
        drift.event_type = Some("flow.object.created".to_string());
        assert!(
            post_flow_delivery_replay(
                State(state),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Json(drift),
            )
            .await
            .is_err(),
            "same key with a changed semantic body must conflict"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_operations_require_exact_confirm_and_keep_dry_runs_canonical_zero_write() {
        let scratch = scratch_or_skip!("v08-maintenance-routes");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        let object_id = create_page_as_owner(&state, workspace_id, owner_id, "maintenance source").await;
        let document_id = document_of(&state, object_id).await;

        let health = body_json(to_response(
            get_flow_admin_health(State(state.clone()), claims_for(owner_id), None, Path(workspace_id)).await,
        ))
        .await;
        assert_eq!(health["code"], 0, "{health}");
        assert!(health["data"]["dead_letter"]["delivery_failed"].is_number());
        assert!(health["data"]["delivery_cancelled"].is_number());
        let lag = body_json(to_response(
            get_flow_admin_lag(State(state.clone()), claims_for(owner_id), None, Path(workspace_id)).await,
        ))
        .await;
        assert_eq!(lag["code"], 0, "{lag}");
        assert_eq!(lag["data"]["projection"]["max"], 0);
        let integrity_summary = body_json(to_response(
            get_flow_admin_integrity(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(workspace_id),
                Query(super::AdminIntegrityQuery {
                    scope: Some("documents".to_string()),
                    cursor: None,
                    limit: Some(50),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(integrity_summary["code"], 0, "{integrity_summary}");
        assert_eq!(integrity_summary["data"]["counts"]["checked"], 2);
        assert!(
            integrity_summary["data"]["documents"]
                .as_array()
                .is_some_and(|documents| documents
                    .iter()
                    .any(|document| document["document_id"] == document_id.to_string())),
            "the complete workspace page must include the requested document: {integrity_summary}"
        );

        let compact_request = CompactDocumentRequest {
            dry_run: true,
            expected_head_seq: Some(0),
            retain_after_seq: None,
            confirm_document_id: None,
            idempotency_key: "compact-dry-key".to_string(),
        };
        let denied = post_flow_compact_document(
            State(state.clone()),
            claims_for(member_id),
            None,
            Path(document_id),
            Json(compact_request.clone()),
        )
        .await;
        assert!(denied.is_err(), "ordinary members cannot run admin maintenance");
        let first = body_json(to_response(
            post_flow_compact_document(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(document_id),
                Json(compact_request.clone()),
            )
            .await,
        ))
        .await;
        let replayed = body_json(to_response(
            post_flow_compact_document(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(document_id),
                Json(compact_request),
            )
            .await,
        ))
        .await;
        assert_eq!(first["code"], 0, "{first}");
        assert_eq!(first["data"]["operation_id"], replayed["data"]["operation_id"]);
        assert!(
            post_flow_compact_document(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(document_id),
                Json(CompactDocumentRequest {
                    dry_run: true,
                    expected_head_seq: Some(1),
                    retain_after_seq: None,
                    confirm_document_id: None,
                    idempotency_key: "compact-dry-key".to_string(),
                }),
            )
            .await
            .is_err(),
            "the same idempotency key with a changed expected head must conflict"
        );
        let snapshot_seq: i64 = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT snapshot_seq FROM collab_documents WHERE id=$1",
                vec![document_id.into()],
            ))
            .await
            .expect("snapshot query runs")
            .expect("document row")
            .try_get("", "snapshot_seq")
            .expect("snapshot seq reads");
        assert_eq!(
            snapshot_seq, 0,
            "compact dry-run must not advance canonical snapshot state"
        );

        let bad_compact = post_flow_compact_document(
            State(state.clone()),
            claims_for(owner_id),
            None,
            Path(document_id),
            Json(CompactDocumentRequest {
                dry_run: false,
                expected_head_seq: Some(0),
                retain_after_seq: Some(0),
                confirm_document_id: Some(Uuid::new_v4()),
                idempotency_key: "compact-bad-confirm".to_string(),
            }),
        )
        .await;
        assert!(
            bad_compact.is_err(),
            "execute must reject a non-matching document confirm"
        );
        let compact_execute = body_json(to_response(
            post_flow_compact_document(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(document_id),
                Json(CompactDocumentRequest {
                    dry_run: false,
                    expected_head_seq: Some(0),
                    retain_after_seq: Some(0),
                    confirm_document_id: Some(document_id),
                    idempotency_key: "compact-execute-key".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(compact_execute["code"], 0, "{compact_execute}");
        assert_eq!(compact_execute["data"]["dry_run"], false);

        let projection = body_json(to_response(
            post_flow_rebuild_projection(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_id),
                Json(RebuildProjectionRequest {
                    dry_run: true,
                    expected_head_seq: Some(0),
                    confirm_object_id: None,
                    idempotency_key: "projection-dry-key".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(projection["code"], 0, "{projection}");
        assert_eq!(projection["data"]["result"]["executed"], false);
        assert!(
            post_flow_rebuild_projection(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_id),
                Json(RebuildProjectionRequest {
                    dry_run: false,
                    expected_head_seq: Some(0),
                    confirm_object_id: Some(Uuid::new_v4()),
                    idempotency_key: "projection-bad-confirm".to_string(),
                }),
            )
            .await
            .is_err()
        );
        exec(
            &state,
            "UPDATE flow_object_projections SET title='CORRUPTED' WHERE object_id=$1",
            vec![object_id.into()],
        )
        .await;
        let projection_execute = body_json(to_response(
            post_flow_rebuild_projection(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(object_id),
                Json(RebuildProjectionRequest {
                    dry_run: false,
                    expected_head_seq: Some(0),
                    confirm_object_id: Some(object_id),
                    idempotency_key: "projection-execute-key".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(projection_execute["code"], 0, "{projection_execute}");
        assert_eq!(projection_execute["data"]["result"]["executed"], true);
        let rebuilt_title: String = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT title FROM flow_object_projections WHERE object_id=$1",
                vec![object_id.into()],
            ))
            .await
            .expect("projection query runs")
            .expect("projection row")
            .try_get("", "title")
            .expect("projection title reads");
        assert_eq!(rebuilt_title, "maintenance source");

        let verified = body_json(to_response(
            post_flow_verify_document(
                State(state.clone()),
                claims_for(owner_id),
                None,
                Path(document_id),
                Json(VerifyDocumentRequest {
                    dry_run: true,
                    deep: true,
                    expected_head_seq: Some(0),
                    idempotency_key: "verify-deep-key".to_string(),
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(verified["code"], 0, "{verified}");
        assert_eq!(
            verified["data"]["result"]["fingerprint"]["document_id"],
            document_id.to_string()
        );

        let operation_rows = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_operation_runs WHERE workspace_id=$1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("operation count runs")
            .expect("operation count row")
            .try_get::<i64>("", "n")
            .expect("operation count reads");
        assert_eq!(
            operation_rows, 5,
            "three successful dry-runs and two exact-confirm executes write operation audit rows"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_package_import_export_upload_preview_commit_status_runs_through_real_http_shapes() {
        use tower::ServiceExt as _;

        let scratch = scratch_or_skip!("package-route-chain");
        let state = state_for(scratch.db.clone());
        let (source_workspace, source_owner) = seed_workspace(&state, true).await;
        let (target_workspace, target_owner) = seed_workspace(&state, true).await;
        create_page_as_owner(&state, source_workspace, source_owner, "route package source").await;

        let source_app = axum::Router::new()
            .route(
                "/api/v1/workspaces/{workspace_id}/flow/exports",
                axum::routing::post(super::post_flow_workspace_export),
            )
            .route(
                "/api/v1/flow/exports/{job_id}",
                axum::routing::get(super::get_flow_export),
            )
            .route(
                "/api/v1/flow/exports/{job_id}/artifact",
                axum::routing::get(super::get_flow_export_artifact),
            )
            .layer(claims_for(source_owner))
            .with_state(state.clone());
        let export_response = source_app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/workspaces/{source_workspace}/flow/exports"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(
                        json!({"format":"package","include_history":false,"idempotency_key":"route-export"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let export_body = body_json(export_response).await;
        assert_eq!(export_body["code"], 0, "{export_body}");
        let export_job_id = export_body["data"]["job_id"].as_str().unwrap();
        let status_response = source_app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/flow/exports/{export_job_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = body_json(status_response).await;
        assert_eq!(status["data"]["checksum"], export_body["data"]["checksum"]);
        let artifact_response = source_app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/flow/exports/{export_job_id}/artifact"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            artifact_response.headers()[axum::http::header::CONTENT_TYPE],
            "application/vnd.sylvode.flow-package+zip;version=1"
        );
        let artifact_bytes = to_bytes(artifact_response.into_body(), usize::MAX).await.unwrap();

        let target_app = axum::Router::new()
            .route(
                "/api/v1/workspaces/{workspace_id}/flow/import-artifacts",
                axum::routing::post(super::post_flow_import_artifact),
            )
            .route(
                "/api/v1/workspaces/{workspace_id}/flow/imports/preview",
                axum::routing::post(super::post_flow_import_preview),
            )
            .route(
                "/api/v1/workspaces/{workspace_id}/flow/imports/{import_id}/commit",
                axum::routing::post(super::post_flow_import_commit),
            )
            .route(
                "/api/v1/workspaces/{workspace_id}/flow/imports/{import_id}",
                axum::routing::get(super::get_flow_import),
            )
            .layer(claims_for(target_owner))
            .with_state(state.clone());
        let upload_response = target_app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/workspaces/{target_workspace}/flow/import-artifacts"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(
                        json!({
                            "source":{"kind":"inline_base64","package_base64":base64::engine::general_purpose::STANDARD.encode(&artifact_bytes)},
                            "idempotency_key":"route-upload"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let upload = body_json(upload_response).await;
        assert_eq!(upload["code"], 0, "{upload}");
        let mut multipart_body = b"--flow-boundary\r\nContent-Disposition: form-data; name=\"package\"; filename=\"flow.sylvode-flow.zip\"\r\nContent-Type: application/vnd.sylvode.flow-package+zip;version=1\r\n\r\n".to_vec();
        multipart_body.extend_from_slice(&artifact_bytes);
        multipart_body.extend_from_slice(b"\r\n--flow-boundary--\r\n");
        let multipart_response = target_app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/workspaces/{target_workspace}/flow/import-artifacts"))
                    .header(
                        axum::http::header::CONTENT_TYPE,
                        "multipart/form-data; boundary=flow-boundary",
                    )
                    .header("idempotency-key", "route-multipart-upload")
                    .body(axum::body::Body::from(multipart_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let multipart_upload = body_json(multipart_response).await;
        assert_eq!(multipart_upload["code"], 0, "{multipart_upload}");
        assert_eq!(
            multipart_upload["data"]["package_sha256"],
            upload["data"]["package_sha256"]
        );
        assert_ne!(multipart_upload["data"]["artifact_id"], upload["data"]["artifact_id"]);
        let preview_response = target_app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/workspaces/{target_workspace}/flow/imports/preview"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(
                        json!({
                            "artifact_id":upload["data"]["artifact_id"],"project_mapping":{},
                            "external_reference_policy":"detach","conflict_policy":"reject_existing",
                            "include_history":false,"idempotency_key":"route-preview"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let preview = body_json(preview_response).await;
        assert_eq!(preview["code"], 0, "{preview}");
        let preview_import_id = preview["data"]["preview_id"].as_str().unwrap();
        let commit_response = target_app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/workspaces/{target_workspace}/flow/imports/{preview_import_id}/commit"
                    ))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(
                        json!({
                            "package_sha256":preview["data"]["package_sha256"],
                            "mapping_hash":preview["data"]["mapping_hash"],"conflict_policy":"reject_existing",
                            "confirm":true,"idempotency_key":"route-commit"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let committed = body_json(commit_response).await;
        assert_eq!(committed["code"], 0, "{committed}");
        let committed_import_job_id = committed["data"]["job_id"].as_str().unwrap();
        let report_response = target_app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/api/v1/workspaces/{target_workspace}/flow/imports/{committed_import_job_id}"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let report = body_json(report_response).await;
        assert_eq!(report["data"]["status"], "completed", "{report}");
        assert_eq!(report["data"]["counts"]["created"], 1);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_package_import_wire_limits_accept_exact_boundary_and_reject_plus_one_with_zero_writes() {
        use tower::ServiceExt as _;

        use crate::flow::import::{ImportLimits, TEST_IMPORT_LIMITS};
        use crate::flow::package::{
            ENGINE_CRATE_VERSION, ENGINE_NAME, ENGINE_WIRE_FORMAT_VERSION, ExportPackageManifest, ExportPolicy,
            PackageCounts, PackageEngine, PackageHistory, PackageMemberInput, PackageProducer, PackageSource,
            build_package,
        };

        const IMPORT_ARTIFACT_ROUTE: &str = "/api/v1/workspaces/{workspace_id}/flow/import-artifacts";

        fn rewrite_snapshot_deflated(package: &[u8]) -> Vec<u8> {
            let mut archive = ZipArchive::new(Cursor::new(package)).expect("stored package opens");
            let mut entries = Vec::new();
            for index in 0..archive.len() {
                let mut member = archive.by_index(index).expect("member opens");
                let mut bytes = Vec::new();
                member.read_to_end(&mut bytes).expect("member reads");
                entries.push((member.name().to_string(), bytes));
            }
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            for (path, bytes) in entries {
                let compression = if path.ends_with("/snapshot.bin") {
                    CompressionMethod::Deflated
                } else {
                    CompressionMethod::Stored
                };
                let options = SimpleFileOptions::default()
                    .compression_method(compression)
                    .large_file(true)
                    .unix_permissions(0o600);
                writer.start_file(path, options).expect("deflated member starts");
                writer.write_all(&bytes).expect("deflated member writes");
            }
            writer.finish().expect("deflated package finishes").into_inner()
        }

        fn package_shape(package: &[u8]) -> (u64, u64) {
            let mut archive = ZipArchive::new(Cursor::new(package)).expect("package opens");
            let entries = u64::try_from(archive.len()).expect("entry count fits");
            let mut expanded = 0u64;
            for index in 0..archive.len() {
                expanded = expanded.saturating_add(archive.by_index(index).expect("member opens").size());
            }
            (entries, expanded)
        }

        fn snapshot_ratio(package: &[u8]) -> u64 {
            let mut archive = ZipArchive::new(Cursor::new(package)).expect("package opens");
            for index in 0..archive.len() {
                let member = archive.by_index(index).expect("member opens");
                if member.name().ends_with("/snapshot.bin") {
                    return member
                        .size()
                        .saturating_add(member.compressed_size().saturating_sub(1))
                        .checked_div(member.compressed_size())
                        .expect("compressed snapshot is nonempty");
                }
            }
            panic!("snapshot member is present");
        }

        fn deterministic_bytes(length: usize) -> Vec<u8> {
            let mut state = 0x9e37_79b9_7f4a_7c15_u64;
            (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state.to_le_bytes()[0]
                })
                .collect()
        }

        async fn upload(
            app: &axum::Router,
            workspace_id: Uuid,
            package: &[u8],
            key: &str,
            limits: ImportLimits,
        ) -> Value {
            TEST_IMPORT_LIMITS
                .scope(limits, async {
                    let response = app
                        .clone()
                        .oneshot(
                            axum::http::Request::builder()
                                .method("POST")
                                .uri(format!("/api/v1/workspaces/{workspace_id}/flow/import-artifacts"))
                                .header(axum::http::header::CONTENT_TYPE, "application/json")
                                .body(axum::body::Body::from(
                                    json!({
                                        "source": {
                                            "kind": "inline_base64",
                                            "package_base64": base64::engine::general_purpose::STANDARD.encode(package),
                                        },
                                        "idempotency_key": key,
                                    })
                                    .to_string(),
                                ))
                                .expect("request builds"),
                        )
                        .await
                        .expect("route responds");
                    body_json(response).await
                })
                .await
        }

        async fn persisted_counts(state: &AppState, workspace_id: Uuid) -> (i64, i64, i64, i64, i64) {
            let row = state
                .db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT \
                       (SELECT count(*) FROM flow_package_artifacts WHERE workspace_id=$1) AS artifacts, \
                       (SELECT count(*) FROM flow_objects WHERE workspace_id=$1) AS objects, \
                       (SELECT count(*) FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id \
                          WHERE o.workspace_id=$1) AS documents, \
                       (SELECT count(*) FROM flow_import_jobs WHERE workspace_id=$1) AS jobs, \
                       (SELECT count(*) FROM business_events WHERE workspace_id=$1) AS events",
                    vec![workspace_id.into()],
                ))
                .await
                .expect("count query runs")
                .expect("count row exists");
            (
                row.try_get("", "artifacts").expect("artifact count"),
                row.try_get("", "objects").expect("object count"),
                row.try_get("", "documents").expect("document count"),
                row.try_get("", "jobs").expect("job count"),
                row.try_get("", "events").expect("event count"),
            )
        }

        fn assert_limit(body: &Value, kind: &str, limit: u64, observed: u64) {
            assert_eq!(body["error_code"], "limit_exceeded", "{body}");
            assert_eq!(body["details"]["limit_kind"], kind, "{body}");
            assert_eq!(body["details"]["limit"], limit, "{body}");
            assert_eq!(body["details"]["observed"], observed, "{body}");
        }

        let scratch = scratch_or_skip!("package-wire-limits");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state, true).await;
        let object_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let manifest = ExportPackageManifest {
            schema: "sylvode.flow.export-package.v1".to_string(),
            package_id: Uuid::new_v4().to_string(),
            created_at: "2026-09-12T12:00:00Z".to_string(),
            producer: PackageProducer {
                product: "sylvode".to_string(),
                version: "0.8.0".to_string(),
                source_head: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
            source: PackageSource {
                workspace_id: Uuid::new_v4().to_string(),
                scope: "object".to_string(),
                root_object_ids: vec![object_id.to_string()],
            },
            flow_schema_version: 1,
            engine: PackageEngine {
                name: ENGINE_NAME.to_string(),
                crate_version: ENGINE_CRATE_VERSION.to_string(),
                wire_format_version: ENGINE_WIRE_FORMAT_VERSION,
            },
            history: PackageHistory {
                included: false,
                through_seq_by_document: std::collections::BTreeMap::from([(document_id.to_string(), 0)]),
            },
            counts: PackageCounts {
                objects: 1,
                documents: 1,
                ..PackageCounts::default()
            },
            members: Vec::new(),
            export_policy: ExportPolicy {
                complete: true,
                permission_snapshot_at: "2026-09-12T12:00:00Z".to_string(),
            },
        };
        let package_with_snapshot = |snapshot: Vec<u8>| {
            build_package(
                manifest.clone(),
                vec![
                    PackageMemberInput {
                        path: format!("documents/{document_id}/snapshot.bin"),
                        kind: "snapshot".to_string(),
                        bytes: snapshot,
                    },
                    PackageMemberInput {
                        path: format!("objects/{object_id}/object.json"),
                        kind: "object".to_string(),
                        bytes: b"{}".to_vec(),
                    },
                    PackageMemberInput {
                        path: "lineage/lineage.jsonl".to_string(),
                        kind: "lineage".to_string(),
                        bytes: Vec::new(),
                    },
                    PackageMemberInput {
                        path: "relations/relations.jsonl".to_string(),
                        kind: "relation".to_string(),
                        bytes: Vec::new(),
                    },
                ],
            )
            .expect("valid stored package builds")
            .bytes
        };
        let exact_base = deterministic_bytes(8 * 1024);
        let package = package_with_snapshot([exact_base.clone(), exact_base].concat());
        let ratio_exact_package = rewrite_snapshot_deflated(&package);
        let plus_one_base = deterministic_bytes(6 * 1024);
        let ratio_plus_one_package = rewrite_snapshot_deflated(&package_with_snapshot(
            plus_one_base.iter().copied().cycle().take(16 * 1024).collect(),
        ));
        assert_eq!(snapshot_ratio(&ratio_exact_package), 2, "exact ratio fixture drifted");
        assert_eq!(snapshot_ratio(&ratio_plus_one_package), 3, "+1 ratio fixture drifted");
        let (entries, expanded) = package_shape(&package);
        let app = axum::Router::new()
            .route(
                IMPORT_ARTIFACT_ROUTE,
                axum::routing::post(super::post_flow_import_artifact),
            )
            .layer(claims_for(owner_id))
            .with_state(state.clone());
        let unrestricted = ImportLimits {
            archive_bytes: u64::MAX,
            expanded_bytes: u64::MAX,
            entry_count: u64::MAX,
            compression_ratio: u64::MAX,
        };

        let cases = [
            (
                "import_archive_bytes",
                ImportLimits {
                    archive_bytes: u64::try_from(package.len()).expect("size fits"),
                    ..unrestricted
                },
                ImportLimits {
                    archive_bytes: u64::try_from(package.len() - 1).expect("size fits"),
                    ..unrestricted
                },
                u64::try_from(package.len() - 1).expect("size fits"),
                u64::try_from(package.len()).expect("size fits"),
            ),
            (
                "import_expanded_bytes",
                ImportLimits {
                    expanded_bytes: expanded,
                    ..unrestricted
                },
                ImportLimits {
                    expanded_bytes: expanded - 1,
                    ..unrestricted
                },
                expanded - 1,
                expanded,
            ),
            (
                "import_entry_count",
                ImportLimits {
                    entry_count: entries,
                    ..unrestricted
                },
                ImportLimits {
                    entry_count: entries - 1,
                    ..unrestricted
                },
                entries - 1,
                entries,
            ),
        ];
        for (index, (kind, exact_limits, plus_one_limits, limit, observed)) in cases.into_iter().enumerate() {
            let exact = upload(
                &app,
                workspace_id,
                &package,
                &format!("{kind}-exact-{index}"),
                exact_limits,
            )
            .await;
            assert_eq!(exact["code"], 0, "exact boundary must pass for {kind}: {exact}");
            let before = persisted_counts(&state, workspace_id).await;
            let plus_one = upload(
                &app,
                workspace_id,
                &package,
                &format!("{kind}-plus-one-{index}"),
                plus_one_limits,
            )
            .await;
            assert_limit(&plus_one, kind, limit, observed);
            assert_eq!(
                persisted_counts(&state, workspace_id).await,
                before,
                "{kind} rejection must write neither staging nor canonical rows"
            );
        }

        let ratio_exact = upload(
            &app,
            workspace_id,
            &ratio_exact_package,
            "import-compression-ratio-exact",
            ImportLimits {
                compression_ratio: 2,
                ..unrestricted
            },
        )
        .await;
        assert_eq!(
            ratio_exact["code"], 0,
            "ratio 2 exact boundary must pass: {ratio_exact}"
        );
        let before = persisted_counts(&state, workspace_id).await;
        let ratio_plus_one = upload(
            &app,
            workspace_id,
            &ratio_plus_one_package,
            "import-compression-ratio-plus-one",
            ImportLimits {
                compression_ratio: 2,
                ..unrestricted
            },
        )
        .await;
        assert_limit(&ratio_plus_one, "import_compression_ratio", 2, 3);
        assert_eq!(
            persisted_counts(&state, workspace_id).await,
            before,
            "compression-ratio rejection must write neither staging nor canonical rows"
        );

        scratch.drop_self().await;
    }
}

// ---------------------------------------------------------------------------------------------
// `ADR-0012` authorization surface
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GrantRequestBody {
    pub principal_kind: String,
    pub principal_id: Uuid,
    pub level: String,
}

impl From<GrantRequestBody> for GrantRequest {
    fn from(body: GrantRequestBody) -> Self {
        Self {
            principal_kind: body.principal_kind,
            principal_id: body.principal_id,
            level: body.level,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SetGrantsRequest {
    pub grants: Vec<GrantRequestBody>,
    #[serde(default)]
    pub confirm_self_lockout: bool,
    #[serde(default)]
    pub dry_run: bool,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize)]
pub struct SetInheritanceRequest {
    pub inherit_from_parent: bool,
    #[serde(default)]
    pub confirm_self_lockout: bool,
    #[serde(default)]
    pub dry_run: bool,
    /// `rest-api-v1.md` spells this field `initial_grants?`: absent means "leave the grants
    /// alone", and is *not* the same request as `initial_grants: []`, which `ADR-0012` §4.1
    /// point 2's replacement semantics make an explicit "clear every explicit grant". Hence
    /// `Option`, not a `#[serde(default)]` `Vec`.
    #[serde(default)]
    pub initial_grants: Option<Vec<GrantRequestBody>>,
    pub idempotency_key: String,
}

/// Resolves the object's workspace, runs the workspace-membership + `flow_enabled` gate, and
/// packages the caller the way `authz::effective_permission` judges principals.
///
/// The object-level `view`/`full_access` check is deliberately *not* here: it belongs to
/// `flow::grants`, which has to run it inside the same transaction it would commit
/// (`ADR-0012` §4.1's post-state rule), not in the handler where it could go stale.
async fn authorization_caller(
    state: &AppState,
    extensions: &axum::http::Extensions,
    object_id: Uuid,
) -> Result<(Uuid, Caller), ApiError> {
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(policy::object_not_found)?;
    let (actor_id, role, is_bot) = require_workspace_access(state, extensions, workspace_id)
        .await
        .map_err(policy::collapse_object_denial)?;
    policy::require_flow_enabled(state, workspace_id).await?;
    Ok((
        workspace_id,
        Caller {
            actor_id,
            principal_kind: if is_bot { "bot".to_string() } else { "user".to_string() },
            role,
            origin: request_origin(extensions),
        },
    ))
}

/// `GET /api/v1/flow/objects/{object_id}/grants`
pub async fn get_flow_object_grants(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (workspace_id, caller) = authorization_caller(&state, &extensions, object_id).await?;
    let view = grants::get_grants(&state, workspace_id, object_id, &caller).await?;
    Ok(ApiResponse::success(view))
}

/// `PUT /api/v1/flow/objects/{object_id}/grants`
pub async fn put_flow_object_grants(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(req): Json<SetGrantsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (workspace_id, caller) = authorization_caller(&state, &extensions, object_id).await?;
    let view = grants::set_grants(
        &state,
        workspace_id,
        SetGrantsInput {
            object_id,
            caller,
            grants: req.grants.into_iter().map(Into::into).collect(),
            confirm_self_lockout: req.confirm_self_lockout,
            dry_run: req.dry_run,
            idempotency_key: req.idempotency_key,
        },
    )
    .await?;
    Ok(ApiResponse::success(view))
}

/// `PUT /api/v1/flow/objects/{object_id}/inheritance`
pub async fn put_flow_object_inheritance(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(req): Json<SetInheritanceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (workspace_id, caller) = authorization_caller(&state, &extensions, object_id).await?;
    let view = grants::set_inheritance(
        &state,
        workspace_id,
        SetInheritanceInput {
            object_id,
            caller,
            inherit_from_parent: req.inherit_from_parent,
            initial_grants: req
                .initial_grants
                .map(|grants| grants.into_iter().map(Into::into).collect()),
            confirm_self_lockout: req.confirm_self_lockout,
            dry_run: req.dry_run,
            idempotency_key: req.idempotency_key,
        },
    )
    .await?;
    Ok(ApiResponse::success(view))
}
