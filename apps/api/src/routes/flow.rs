//! HTTP handlers for the four Flow REST endpoints this package ships.
//!
//! `rest-api-v1.md` "v0.4 Flow Alpha", minus `bootstrap`/`commands`/`collab`/`collab/verify`/the
//! WebSocket ticket pair, which are a later package — see `apps/api/src/flow/mod.rs`'s module
//! docs.
//!
//! ```text
//! POST /api/v1/workspaces/{workspace_id}/flow/objects
//! GET  /api/v1/workspaces/{workspace_id}/flow/objects
//! GET  /api/v1/flow/objects/{object_id}
//! GET  /api/v1/flow/objects/{object_id}/history
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
use uuid::Uuid;

use crate::middleware::bot_auth::BotAuthContext;
use crate::{
    error::ApiError,
    flow::{command::CreateObjectInput, policy, query, query::Render},
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
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use serde_json::Value;
    use uuid::Uuid;

    use super::{
        CreateFlowObjectRequest, FlowObjectHistoryQuery, GetFlowObjectQuery, ListFlowObjectsQuery, create_flow_object,
        get_flow_object, get_flow_object_history, list_flow_objects,
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
}
