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
        policy, query,
        query::Render,
    },
    response::ApiResponse,
};

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
    let (actor_id, _role, _is_bot) = policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    let accepted = crate::flow::command::create_object(
        &state,
        CreateObjectInput {
            workspace_id,
            actor_id,
            object_type: req.object_type,
            project_id: req.project_id,
            parent_object_id: req.parent_object_id,
            title: req.title,
            idempotency_key: req.idempotency_key,
            message: req.message,
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
    policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    let response = query::list_objects(
        &state,
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
    policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    let render = Render::parse(params.render.as_deref())?;
    let view = query::get_object(&state, object_id, params.at_seq, render).await?;

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
    policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    let bootstrap = query::get_bootstrap(&state, object_id, params.known_seq, params.known_frontier).await?;

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
/// update_block|delete_block|move_block|archive|restore`).
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
    policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    let response = query::get_history(&state, object_id, params.before_seq, params.limit).await?;

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
    let (actor_id, _role, _is_bot) =
        policy::require_flow_workspace_admin_access(&state, &extensions, workspace_id).await?;

    let view = crate::flow::command::set_flow_feature(
        &state,
        SetFlowFeatureInput {
            workspace_id,
            actor_id,
            enabled: req.enabled,
            default_member_level: req.default_member_level,
            idempotency_key: req.idempotency_key,
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
        list_flow_objects, post_flow_object_command, set_flow_feature,
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

    /// `POST /api/v1/flow/objects/{object_id}/commands`: all seven v0.4 command types, each
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
        let message = plus_one_body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("semantic_patch_operations"),
            "the rejection message must name limit_kind=semantic_patch_operations: {plus_one_body}"
        );

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
}
