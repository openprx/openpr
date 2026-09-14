//! v0.9 full-workspace package round trip through the production MCP tool dispatch.
#![allow(clippy::indexing_slicing)]
#![allow(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "gate harness emits one machine-readable result and an explicit unavailable-environment notice"
)]

use std::{collections::BTreeMap, error::Error, time::Duration};

use api::{
    flow::{
        command::{
            CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command,
            set_flow_feature,
        },
        event_origin::{CommandOrigin, EventSurface},
    },
    middleware::bot_auth::bot_or_user_auth_middleware,
    routes::flow::{
        get_flow_export, get_flow_export_artifact, get_flow_import, post_flow_import_artifact, post_flow_import_commit,
        post_flow_import_preview, post_flow_workspace_export,
    },
};
use axum::{
    Router, middleware,
    routing::{get, post},
};
use collab_core::{CollabEngine, LoroCollabEngine};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    protocol::{CallToolResult, ToolContent},
    server::McpServer,
};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn create() -> Result<Option<Self>, Box<dyn Error>> {
        let Ok(admin_url) = std::env::var(TEST_DATABASE_URL_ENV) else {
            return Ok(None);
        };
        let admin = Database::connect(&admin_url).await?;
        let name = format!("openpr_v09_roundtrip_{}", Uuid::new_v4().simple());
        admin.execute_unprepared(&format!("CREATE DATABASE \"{name}\"")).await?;
        let (prefix, _) = admin_url
            .rsplit_once('/')
            .ok_or("test database URL has no database component")?;
        drop(admin);
        let mut options = ConnectOptions::new(format!("{prefix}/{name}"));
        options.acquire_timeout(Duration::from_mins(3));
        let db = Database::connect(options).await?;
        let mut migrations: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect();
        migrations.sort();
        for path in migrations {
            db.execute_unprepared(&std::fs::read_to_string(path)?).await?;
        }
        Ok(Some(Self { db, name, admin_url }))
    }

    async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        if let Ok(admin) = Database::connect(admin_url).await {
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "v09-roundtrip-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("v09-roundtrip-secret"),
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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> Result<(), Box<dyn Error>> {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    Ok(())
}

async fn seed_workspace(state: &AppState, label: &str, token: &str) -> Result<(Uuid, Uuid), Box<dyn Error>> {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        &state.db,
        "INSERT INTO users (id,email,password_hash,name,role,is_active) VALUES ($1,$2,'!',$3,'user',true)",
        vec![
            owner_id.into(),
            format!("{owner_id}@roundtrip.test").into(),
            label.into(),
        ],
    )
    .await?;
    exec(
        &state.db,
        "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,$3,$4)",
        vec![
            workspace_id.into(),
            format!("v09-{workspace_id}").into(),
            label.into(),
            owner_id.into(),
        ],
    )
    .await?;
    exec(
        &state.db,
        "INSERT INTO workspace_members (workspace_id,user_id,role) VALUES ($1,$2,'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await?;
    set_flow_feature(
        state,
        SetFlowFeatureInput {
            workspace_id,
            actor_id: owner_id,
            actor_is_bot: false,
            enabled: Some(true),
            default_member_level: Some("edit".to_string()),
            idempotency_key: format!("enable-{label}"),
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        },
    )
    .await?;
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    exec(&state.db, "INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,transport_surface) VALUES ($1,$2,$3,$4,$5,$6,$7,'mcp_stdio')",
         vec![Uuid::new_v4().into(), workspace_id.into(), format!("{label} bot").into(), token_hash.into(), token[..8].into(), json!(["read","write","admin"]).into(), owner_id.into()]).await?;
    Ok((workspace_id, owner_id))
}

async fn seed_source_graph(state: &AppState, workspace_id: Uuid, owner_id: Uuid) -> Result<Vec<Uuid>, Box<dyn Error>> {
    let mut objects = Vec::new();
    for index in 0..4 {
        let object = create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: objects.last().copied(),
                title: format!("round trip page {index}"),
                idempotency_key: format!("roundtrip-create-{index}"),
                message: Some("v0.9 fixture".to_string()),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await?
        .object
        .id;
        execute_command(
            state,
            ExecuteCommandInput {
                object_id: object,
                actor_id: owner_id,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "set_title".to_string(),
                payload: json!({"title":format!("accepted round trip page {index}")}),
                expected_frontier: None,
                idempotency_key: format!("roundtrip-update-{index}"),
                message: None,
                origin_client_id: "v09-roundtrip-fixture".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await?;
        objects.push(object);
    }
    execute_command(state, ExecuteCommandInput {
        object_id: objects[0], actor_id: owner_id, principal_kind: "user".to_string(), role: "owner".to_string(),
        command_type: "link".to_string(), payload: json!({"target_object_id":objects[1],"relation_type":"references","properties":{"label":"v0.9 edge"},"position_key":"a0"}),
        expected_frontier: None, idempotency_key: "roundtrip-link".to_string(), message: None,
        origin_client_id: "v09-roundtrip-fixture".to_string(),
        origin: CommandOrigin::first_request_from(EventSurface::Rest),
    }).await?;
    Ok(objects)
}

fn package_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/exports",
            post(post_flow_workspace_export),
        )
        .route("/api/v1/flow/exports/{job_id}", get(get_flow_export))
        .route("/api/v1/flow/exports/{job_id}/artifact", get(get_flow_export_artifact))
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/import-artifacts",
            post(post_flow_import_artifact),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/imports/preview",
            post(post_flow_import_preview),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/imports/{import_id}/commit",
            post(post_flow_import_commit),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/imports/{import_id}",
            get(get_flow_import),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            bot_or_user_auth_middleware,
        ))
        .with_state(state)
}

fn result_json(result: &CallToolResult) -> Result<Value, Box<dyn Error>> {
    if result.is_error.is_some() {
        return Err(format!("MCP tool failed: {:?}", result.content).into());
    }
    let Some(ToolContent::Text { text }) = result.content.first() else {
        return Err("MCP result has no text".into());
    };
    Ok(serde_json::from_str(text)?)
}

#[derive(Debug, FromQueryResult)]
struct DocumentState {
    object_id: Uuid,
    document_id: Uuid,
    object_type: String,
    lifecycle_status: String,
    parent_id: Option<Uuid>,
    governance_metadata: Value,
    head_seq: i64,
    head_frontier: Vec<u8>,
    snapshot: Vec<u8>,
    snapshot_seq: i64,
    projection_seq: i64,
    projection_frontier: Vec<u8>,
}

async fn document_states(db: &DatabaseConnection, workspace_id: Uuid) -> Result<Vec<DocumentState>, Box<dyn Error>> {
    Ok(DocumentState::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT fo.id AS object_id,cd.id AS document_id,fo.object_type,fo.lifecycle_status,fo.parent_id,fo.governance_metadata,cd.head_seq,cd.head_frontier,cd.snapshot,cd.snapshot_seq,fp.document_seq AS projection_seq,fp.document_frontier AS projection_frontier FROM flow_objects fo JOIN collab_documents cd ON cd.object_id=fo.id JOIN flow_object_projections fp ON fp.object_id=fo.id WHERE fo.workspace_id=$1 AND NOT flow_is_system_navigator_root(fo.object_type,fo.parent_id,fo.governance_metadata) ORDER BY fo.id",
        vec![workspace_id.into()])).all(db).await?)
}

async fn semantic_hash(db: &DatabaseConnection, state: &DocumentState) -> Result<String, Box<dyn Error>> {
    #[derive(Debug, FromQueryResult)]
    struct Update {
        bytes: Vec<u8>,
    }
    let mut engine = LoroCollabEngine::load(&state.snapshot)?;
    let updates = Update::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT bytes FROM collab_updates WHERE document_id=$1 AND seq>$2 AND seq<=$3 ORDER BY seq",
        vec![
            state.document_id.into(),
            state.snapshot_seq.into(),
            state.head_seq.into(),
        ],
    ))
    .all(db)
    .await?;
    for update in updates {
        engine.import_update(&update.bytes)?;
    }
    Ok(engine.semantic_snapshot()?.semantic_hash())
}

async fn canonical_counts(db: &DatabaseConnection, workspace_id: Uuid) -> Result<Value, Box<dyn Error>> {
    let row = db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT (SELECT count(*) FROM flow_objects WHERE workspace_id=$1) AS objects,(SELECT count(*) FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE o.workspace_id=$1) AS documents,(SELECT count(*) FROM flow_relations WHERE workspace_id=$1) AS relations,(SELECT count(*) FROM collab_updates u JOIN collab_documents d ON d.id=u.document_id JOIN flow_objects o ON o.id=d.object_id WHERE o.workspace_id=$1) AS updates,(SELECT coalesce(sum(head_seq),0)::bigint FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE o.workspace_id=$1) AS head_seq_sum",
        vec![workspace_id.into()])).await?.ok_or("count row missing")?;
    Ok(
        json!({"objects":row.try_get::<i64>("","objects")?,"documents":row.try_get::<i64>("","documents")?,"relations":row.try_get::<i64>("","relations")?,"updates":row.try_get::<i64>("","updates")?,"head_seq_sum":row.try_get::<i64>("","head_seq_sum")?}),
    )
}

#[tokio::test]
async fn full_workspace_roundtrip_compares_every_document_graph_lineage_and_report_through_mcp()
-> Result<(), Box<dyn Error>> {
    let Some(scratch) = Scratch::create().await? else {
        eprintln!("SKIPPED (no database): set {TEST_DATABASE_URL_ENV} to run this test");
        return Ok(());
    };
    let state = state_for(scratch.db.clone());
    let source_token = "opr_v09_source_roundtrip_token";
    let target_token = "opr_v09_target_roundtrip_token";
    let (source_workspace, source_owner) = seed_workspace(&state, "source", source_token).await?;
    let (target_workspace, _) = seed_workspace(&state, "target", target_token).await?;
    seed_source_graph(&state, source_workspace, source_owner).await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(listener, package_router(state)).await;
    });
    let source = McpServer::new(OpenPrClient::new(ClientConfig {
        base_url: format!("http://{address}"),
        credential: Some(Secret::new(source_token)),
        workspace_id: source_workspace.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?);
    let target = McpServer::new(OpenPrClient::new(ClientConfig {
        base_url: format!("http://{address}"),
        credential: Some(Secret::new(target_token)),
        workspace_id: target_workspace.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?);

    let exported = result_json(
        &source
            .call_tool(
                "objects.export_workspace",
                json!({"workspace_id":source_workspace,"include_history":true,"idempotency_key":"v09-full-export"}),
            )
            .await,
    )?;
    let job_id = exported["job_id"].as_str().ok_or("export job id missing")?;
    let package = reqwest::Client::new()
        .get(format!("http://{address}/api/v1/flow/exports/{job_id}/artifact"))
        .bearer_auth(source_token)
        .header("x-openpr-mcp-surface", TRANSPORT_LABEL_STDIO)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let package_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &package);

    let upload = result_json(&target.call_tool("objects.import_artifact", json!({"workspace_id":target_workspace,"package_base64":package_base64,"package_sha256":exported["checksum"],"idempotency_key":"v09-artifact"})).await)?;
    let preview = result_json(&target.call_tool("objects.import_preview", json!({"workspace_id":target_workspace,"artifact_id":upload["artifact_id"],"project_mapping":{},"external_reference_policy":"detach","conflict_policy":"reject_existing","include_history":true,"idempotency_key":"v09-preview"})).await)?;
    let committed = result_json(&target.call_tool("objects.import_commit", json!({"workspace_id":target_workspace,"import_id":preview["preview_id"],"package_sha256":preview["package_sha256"],"mapping_hash":preview["mapping_hash"],"conflict_policy":"reject_existing","confirm":true,"idempotency_key":"v09-commit"})).await)?;
    let report = result_json(
        &target
            .call_tool(
                "objects.import_status",
                json!({"workspace_id":target_workspace,"import_id":committed["import_id"]}),
            )
            .await,
    )?;
    assert_eq!(report["status"], "completed");

    let object_mapping: BTreeMap<Uuid, Uuid> = serde_json::from_value(report["object_mapping"].clone())?;
    let document_mapping: BTreeMap<Uuid, Uuid> = serde_json::from_value(report["document_mapping"].clone())?;
    let source_states = document_states(&scratch.db, source_workspace).await?;
    let target_states = document_states(&scratch.db, target_workspace).await?;
    assert_eq!(object_mapping.len(), source_states.len());
    assert_eq!(document_mapping.len(), source_states.len());
    assert_eq!(target_states.len(), source_states.len());
    let target_by_object: BTreeMap<_, _> = target_states.iter().map(|state| (state.object_id, state)).collect();
    let mut parent_edges = Vec::with_capacity(source_states.len());
    for source_state in &source_states {
        let target_state = target_by_object[&object_mapping[&source_state.object_id]];
        assert_eq!(document_mapping[&source_state.document_id], target_state.document_id);
        assert_eq!(source_state.object_type, target_state.object_type);
        assert_eq!(source_state.lifecycle_status, target_state.lifecycle_status);
        assert_eq!(source_state.governance_metadata, target_state.governance_metadata);
        let source_parent = source_state.parent_id.filter(|id| object_mapping.contains_key(id));
        let expected_target_parent = source_parent.and_then(|id| object_mapping.get(&id).copied());
        let actual_target_parent = target_state.parent_id.filter(|id| target_by_object.contains_key(id));
        assert_eq!(expected_target_parent, actual_target_parent);
        parent_edges.push(json!({
            "source_object_id":source_state.object_id,
            "source_parent_id":source_parent,
            "target_object_id":target_state.object_id,
            "expected_target_parent_id":expected_target_parent,
            "target_parent_id":actual_target_parent,
        }));
        assert_eq!(source_state.head_seq, target_state.head_seq);
        assert_eq!(source_state.head_frontier, target_state.head_frontier);
        assert_eq!(source_state.projection_seq, target_state.projection_seq);
        assert_eq!(source_state.projection_frontier, target_state.projection_frontier);
        assert_eq!(
            semantic_hash(&scratch.db, source_state).await?,
            semantic_hash(&scratch.db, target_state).await?
        );
    }
    assert_eq!(
        parent_edges
            .iter()
            .filter(|edge| !edge["source_parent_id"].is_null())
            .count(),
        3,
        "the roundtrip fixture must contain the three non-root edges in root -> A -> B -> C"
    );

    let relation_rows = scratch.db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT source_id,target_id FROM flow_import_lineage WHERE target_workspace_id=$1 AND package_sha256=$2 AND source_kind='flow_package' AND target_kind='relation'",
        vec![target_workspace.into(), upload["package_sha256"].as_str().ok_or("package hash missing")?.into()])).await?;
    assert_eq!(relation_rows.len(), 1);
    let source_relation: Uuid = relation_rows[0].try_get("", "source_id")?;
    let target_relation: Uuid = relation_rows[0].try_get("", "target_id")?;
    let graph = scratch.db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT s.relation_type=t.relation_type AND s.properties=t.properties AND s.position_key IS NOT DISTINCT FROM t.position_key AND source_map.target_id=t.source_object_id AND target_map.target_id=t.target_object_id AS same FROM flow_relations s JOIN flow_relations t ON t.id=$2 JOIN flow_import_lineage source_map ON source_map.target_workspace_id=$3 AND source_map.package_sha256=$4 AND source_map.source_kind='flow_package' AND source_map.target_kind='object' AND source_map.source_id=s.source_object_id JOIN flow_import_lineage target_map ON target_map.target_workspace_id=$3 AND target_map.package_sha256=$4 AND target_map.source_kind='flow_package' AND target_map.target_kind='object' AND target_map.source_id=s.target_object_id WHERE s.id=$1",
        vec![source_relation.into(),target_relation.into(),target_workspace.into(),upload["package_sha256"].as_str().ok_or("package hash missing")?.into()])).await?.ok_or("relation comparison missing")?;
    assert!(graph.try_get::<bool>("", "same")?);

    let lineage_count: i64 = scratch.db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT count(*) AS n FROM flow_import_lineage WHERE target_workspace_id=$1 AND package_sha256=$2 AND source_kind='flow_package'",
        vec![target_workspace.into(), upload["package_sha256"].as_str().ok_or("package hash missing")?.into()])).await?.ok_or("lineage count missing")?.try_get("", "n")?;
    assert_eq!(lineage_count, i64::try_from(source_states.len() * 2 + 1)?);
    assert_eq!(report["counts"]["planned"], json!(source_states.len()));
    assert_eq!(report["counts"]["created"], json!(source_states.len()));
    assert_eq!(report["counts"]["reused"], 0);

    let before_reuse = canonical_counts(&scratch.db, target_workspace).await?;
    let reuse_preview = result_json(&target.call_tool("objects.import_preview", json!({"workspace_id":target_workspace,"artifact_id":upload["artifact_id"],"project_mapping":{},"external_reference_policy":"detach","conflict_policy":"reuse_import_lineage","include_history":true,"idempotency_key":"v09-reuse-preview"})).await)?;
    let reuse_commit = result_json(&target.call_tool("objects.import_commit", json!({"workspace_id":target_workspace,"import_id":reuse_preview["preview_id"],"package_sha256":reuse_preview["package_sha256"],"mapping_hash":reuse_preview["mapping_hash"],"conflict_policy":"reuse_import_lineage","confirm":true,"idempotency_key":"v09-reuse-commit"})).await)?;
    let reuse_report = result_json(
        &target
            .call_tool(
                "objects.import_status",
                json!({"workspace_id":target_workspace,"import_id":reuse_commit["import_id"]}),
            )
            .await,
    )?;
    let after_reuse = canonical_counts(&scratch.db, target_workspace).await?;
    assert_eq!(
        before_reuse, after_reuse,
        "reuse_import_lineage must perform zero canonical writes and head changes"
    );
    assert_eq!(reuse_report["object_mapping"], report["object_mapping"]);
    assert_eq!(reuse_report["document_mapping"], report["document_mapping"]);
    assert_eq!(reuse_report["counts"]["created"], 0);
    assert_eq!(reuse_report["counts"]["reused"], json!(source_states.len()));

    println!(
        "FLOW_V09_ROUNDTRIP_RESULT={}",
        json!({
            "status":"passed","package_schema":"v1","source_workspace":source_workspace,"target_workspace":target_workspace,
            "documents_enumerated":source_states.len(),"documents_compared":source_states.len(),"objects_compared":object_mapping.len(),
            "relations_compared":1,"lineage_rows_compared":lineage_count,"report_checksum":hex::encode(Sha256::digest(serde_jcs::to_vec(&report)?)),
            "comparison":{"head_seq":true,"head_frontier":true,"semantic_hash":true,"projection_seq":true,"projection_frontier":true,"object_metadata":true,"parent_graph":true,"relation_graph":true,"lineage":true,"import_report":true},
            "parent_edges":parent_edges,
            "mcp_import_chain":["objects.import_artifact","objects.import_preview","objects.import_commit","objects.import_status"],
            "command_trace":[
                {"ordinal":1,"tool":"objects.import_artifact"},
                {"ordinal":2,"tool":"objects.import_preview","conflict_policy":"reject_existing"},
                {"ordinal":3,"tool":"objects.import_commit","conflict_policy":"reject_existing","existing_document_cardinality":0,"canonical_writes":source_states.len(),"head_changes":source_states.len()},
                {"ordinal":4,"tool":"objects.import_status"},
                {"ordinal":5,"tool":"objects.import_preview","conflict_policy":"reuse_import_lineage"},
                {"ordinal":6,"tool":"objects.import_commit","conflict_policy":"reuse_import_lineage","existing_document_cardinality":0,"canonical_writes":0,"head_changes":0},
                {"ordinal":7,"tool":"objects.import_status"}
            ],
            "branches":[
                {"conflict_policy":"reject_existing","existing_document_cardinality":0,"canonical_writes":source_states.len(),"head_changes":source_states.len()},
                {"conflict_policy":"reuse_import_lineage","existing_document_cardinality":0,"canonical_writes":0,"head_changes":0,"before":before_reuse,"after":after_reuse}
            ]
        })
    );
    server_task.abort();
    scratch.drop_self().await;
    Ok(())
}
