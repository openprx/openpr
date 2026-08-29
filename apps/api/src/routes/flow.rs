//! HTTP handlers for the Flow REST endpoints this package ships.
//!
//! `rest-api-v1.md` "v0.4 Flow Alpha", minus `bootstrap`/`collab`/`collab/verify`/the WebSocket
//! ticket pair, which are a later package — see `apps/api/src/flow/mod.rs`'s module docs.
//!
//! ```text
//! POST /api/v1/workspaces/{workspace_id}/flow/objects
//! GET  /api/v1/workspaces/{workspace_id}/flow/objects
//! GET  /api/v1/flow/objects/{object_id}
//! POST /api/v1/flow/objects/{object_id}/commands
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
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        CreateFlowObjectRequest, ExecuteFlowCommandRequest, FlowCommandEnvelope, FlowObjectHistoryQuery,
        GetFlowObjectQuery, ListFlowObjectsQuery, SetFlowFeatureRequest, create_flow_object, get_flow_feature,
        get_flow_object, get_flow_object_history, list_flow_objects, post_flow_object_command, set_flow_feature,
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
}
