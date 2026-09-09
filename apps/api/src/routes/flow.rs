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
//! GET  /api/v1/flow/objects/{object_id}/bootstrap
//! GET  /api/v1/flow/objects/{object_id}/grants
//! PUT  /api/v1/flow/objects/{object_id}/grants
//! PUT  /api/v1/flow/objects/{object_id}/inheritance
//! GET  /api/v1/flow/objects/{object_id}/history
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
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::middleware::bot_auth::BotAuthContext;
use crate::{
    error::ApiError,
    flow::{
        command::{CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput},
        event_origin::{CommandOrigin, EventSource, EventSurface},
        grants::{self, Caller, GrantRequest, SetGrantsInput, SetInheritanceInput},
        policy, query,
        query::Render,
    },
    response::ApiResponse,
};

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

#[derive(Debug, Deserialize)]
pub struct CreateFlowObjectRequest {
    pub object_type: String,
    pub project_id: Option<Uuid>,
    pub parent_object_id: Option<Uuid>,
    pub title: String,
    pub idempotency_key: String,
    pub message: Option<String>,
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

    let accepted = crate::flow::command::create_object(
        &state,
        CreateObjectInput {
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
        },
    )
    .await?;

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
    let access = policy::begin_flow_read(&state, &extensions, workspace_id).await?;

    let response = query::list_objects(
        &state,
        &access,
        query::ListObjectsParams {
            workspace_id,
            project_id: params.project_id,
            unprojected: params.unprojected,
            object_type: params.object_type,
            parent_id: params.parent_id,
            q: params.q,
            cursor: params.cursor,
            limit: params.limit,
            include_archived: params.include_archived,
        },
    )
    .await?;

    Ok(ApiResponse::success(response))
}

#[derive(Debug, Deserialize)]
pub struct GetFlowObjectQuery {
    pub at_seq: Option<i64>,
    pub render: Option<String>,
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
    let access = policy::require_flow_object_access(
        &state,
        &extensions,
        workspace_id,
        object_id,
        crate::flow::collab::authz::PermissionLevel::View,
    )
    .await?;

    let render = Render::parse(params.render.as_deref())?;
    let view = query::get_object(&state, &access, params.at_seq, render).await?;

    Ok(ApiResponse::success(view))
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
    let access = policy::require_flow_object_access(
        &state,
        &extensions,
        workspace_id,
        object_id,
        crate::flow::collab::authz::PermissionLevel::Edit,
    )
    .await?;

    let bootstrap = query::get_bootstrap(&state, &access, params.known_seq, params.known_frontier).await?;

    Ok(ApiResponse::success(bootstrap))
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

/// `POST /api/v1/flow/objects/{object_id}/commands` (`rest-api-v1.md`: `set_title|insert_block|
/// update_block|delete_block|move_block|semantic_patch|archive|restore`).
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
    let access = policy::require_flow_object_access(
        &state,
        &extensions,
        workspace_id,
        object_id,
        crate::flow::collab::authz::PermissionLevel::View,
    )
    .await?;

    let response = query::get_history(&state, &access, params.before_seq, params.limit).await?;

    Ok(ApiResponse::success(response))
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod flow_database_tests {
    use super::{GrantRequestBody, SetGrantsRequest};
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

    use super::{
        CreateFlowObjectRequest, ExecuteFlowCommandRequest, FlowCommandEnvelope, FlowObjectHistoryQuery,
        GetFlowObjectBootstrapQuery, GetFlowObjectQuery, ListFlowObjectsQuery, SetFlowFeatureRequest,
        create_flow_object, get_flow_feature, get_flow_object, get_flow_object_bootstrap, get_flow_object_history,
        list_flow_objects, post_flow_object_command, put_flow_object_grants, put_flow_object_inheritance,
        set_flow_feature,
    };
    use crate::error::ApiError;
    use axum::extract::{Path, Query, State};
    use axum::{Extension, Json};

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

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
                }),
            )
            .await,
        ))
        .await;
        assert_eq!(body["code"], 0, "{body}");
        Uuid::parse_str(body["data"]["object"]["id"].as_str().expect("object id is a string"))
            .expect("object id is a uuid")
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
                "SELECT count(*) AS n FROM flow_objects WHERE workspace_id = $1",
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
        assert!(list["data"].get("total").is_none(), "{list}");
        assert!(list["data"].get("filtered_count").is_none(), "{list}");
        assert!(list["data"].get("examined").is_none(), "{list}");

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

    /// 1,001 hidden rows force the overfetch loop one row past the public scan ceiling. The error
    /// must reject rather than return a misleading empty short page, and both numeric fields must
    /// expose only the fixed ceiling, never the actual pre-filter count.
    #[tokio::test]
    async fn authorized_overfetch_rejects_past_scan_budget_without_revealing_examined_rows() {
        let scratch = scratch_or_skip!("authorized_scan_budget");
        let state = state_for(scratch.db.clone());
        let (workspace_id, _owner_id) = seed_workspace(&state, true).await;
        let member_id = seed_member(&state, workspace_id).await;
        exec(
            &state,
            "WITH objects AS ( \
                 INSERT INTO flow_objects (id, workspace_id, object_type, inherit_from_parent, created_at) \
                 SELECT gen_random_uuid(), $1, 'page', false, now() + n * interval '1 microsecond' \
                   FROM generate_series(1, 1001) AS n RETURNING id \
             ), documents AS ( \
                 INSERT INTO collab_documents (object_id, format_version, snapshot, snapshot_frontier, head_frontier) \
                 SELECT id, 'loro-1', '\\x'::bytea, '\\x'::bytea, '\\x'::bytea FROM objects \
             ) \
             INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, state, plain_text) \
             SELECT id, 0, '\\x'::bytea, 'hidden', '{}'::jsonb, '' FROM objects",
            vec![workspace_id.into()],
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

    #[tokio::test]
    async fn repeating_the_same_idempotency_key_replays_the_original_object_instead_of_conflicting() {
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
            create_flow_object(State(state.clone()), claims, None, Path(workspace_id), Json(request())).await,
        ))
        .await;
        assert_eq!(second["code"], 0, "{second}");
        assert_eq!(
            second["data"]["object"]["id"], first_id,
            "replay must return the original object id"
        );

        // Only one row was ever written, not two.
        let count = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_objects WHERE workspace_id = $1",
                vec![workspace_id.into()],
            ))
            .await
            .expect("count query runs")
            .expect("count query returns a row");
        let n: i64 = count.try_get("", "n").expect("count column reads");
        assert_eq!(n, 1);

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
    async fn put_with_a_non_edit_default_member_level_is_rejected_via_body_code() {
        let scratch = scratch_or_skip!("feature-put-bad-level");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_bare_workspace(&state).await;
        let claims = claims_for(owner_id);

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
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, is_active) \
             VALUES ($1, $2, 'attribution-bot', $3, $4, '[\"read\",\"write\",\"admin\"]'::jsonb, true)",
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
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let (actor_id, role, is_bot) = policy::require_flow_workspace_access(state, extensions, workspace_id).await?;
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
