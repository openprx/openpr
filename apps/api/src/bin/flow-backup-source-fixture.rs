//! Provisions the bounded `PostgreSQL` source used by the Flow backup/restore gate.

#![allow(clippy::print_stdout)]

use api::flow::collab::authz;
use api::flow::collab::cache::WarmCache;
use api::flow::collab::coordinator::DocumentCoordinator;
use api::flow::collab::integrity::all_document_fingerprints;
use api::flow::collab::registry::SessionRegistry;
use api::flow::collab::snapshot::SnapshotAdvancer;
use api::flow::collab::write::{AcceptOutcome, UpdateRequest, accept_update};
use api::flow::event_origin::{CommandOrigin, EventSurface};
use collab_core::{CollabEngine, LoroCollabEngine};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

const DATABASE_NAME: &str = "openpr_v10_backup_source";
const ADMIN_URL_ENV: &str = "OPENPR_BACKUP_RESTORE_ADMIN_URL";
const SOURCE_URL_ENV: &str = "OPENPR_BACKUP_SOURCE_DATABASE_URL";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cleanup_only = std::env::args().skip(1).any(|arg| arg == "--cleanup");
    let admin_url = std::env::var(ADMIN_URL_ENV).map_err(|_| anyhow::anyhow!("{ADMIN_URL_ENV} is required"))?;
    let admin = Database::connect(&admin_url).await?;
    reset_database(&admin, cleanup_only).await?;
    admin.close().await?;
    if cleanup_only {
        println!("{}", json!({"database_name": DATABASE_NAME, "status": "removed"}));
        return Ok(());
    }

    let source_url = std::env::var(SOURCE_URL_ENV).map_err(|_| anyhow::anyhow!("{SOURCE_URL_ENV} is required"))?;
    let db = Database::connect(&source_url).await?;
    require_expected_database(&db).await?;
    migrate(&db).await?;

    let state = state_for(db.clone());
    let (workspace_id, actor_id) = seed_workspace(&db).await?;
    let first = create_page(&state, workspace_id, actor_id, "Backup fixture one").await?;
    let _second = create_page(&state, workspace_id, actor_id, "Backup fixture two").await?;
    add_retained_update(&db, first, workspace_id, actor_id).await?;

    let fingerprints = all_document_fingerprints(&db).await?;
    anyhow::ensure!(fingerprints.len() >= 2, "expected at least two document fingerprints");
    let tail_updates = scalar_i64(
        &db,
        "SELECT count(*)::bigint AS value FROM collab_updates WHERE seq > 0",
    )
    .await?;
    anyhow::ensure!(tail_updates > 0, "expected at least one retained update");
    println!(
        "{}",
        json!({
            "database_name": DATABASE_NAME,
            "document_count": fingerprints.len(),
            "retained_update_count": tail_updates,
            "status": "ready"
        })
    );
    db.close().await?;
    Ok(())
}

async fn reset_database(admin: &DatabaseConnection, cleanup_only: bool) -> anyhow::Result<()> {
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{DATABASE_NAME}\" WITH (FORCE)"))
        .await?;
    if !cleanup_only {
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{DATABASE_NAME}\""))
            .await?;
    }
    Ok(())
}

#[derive(FromQueryResult)]
struct TextValue {
    value: String,
}

async fn require_expected_database(db: &DatabaseConnection) -> anyhow::Result<()> {
    let row = TextValue::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT current_database() AS value".to_string(),
    ))
    .one(db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("current_database() returned no row"))?;
    anyhow::ensure!(
        row.value == DATABASE_NAME,
        "{SOURCE_URL_ENV} must select the dedicated {DATABASE_NAME} database"
    );
    Ok(())
}

async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut paths: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();
    paths.sort();
    anyhow::ensure!(!paths.is_empty(), "no migrations found in {dir}");
    for path in paths {
        db.execute_unprepared(&std::fs::read_to_string(&path)?)
            .await
            .map_err(|error| anyhow::anyhow!("applying {} failed: {error}", path.display()))?;
    }
    Ok(())
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "flow-backup-source-fixture".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("backup-source-fixture-secret"),
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

async fn execute(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    Ok(())
}

async fn seed_workspace(db: &DatabaseConnection) -> anyhow::Result<(Uuid, Uuid)> {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    execute(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'backup fixture', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@backup.test").into()],
    )
    .await?;
    execute(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'backup fixture', $3)",
        vec![
            workspace_id.into(),
            format!("ws-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await?;
    execute(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await?;
    execute(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
        vec![workspace_id.into()],
    )
    .await?;
    Ok((workspace_id, owner_id))
}

async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid, title: &str) -> anyhow::Result<Uuid> {
    use api::flow::command::{CreateObjectInput, create_object};
    let accepted = create_object(
        state,
        CreateObjectInput {
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
            workspace_id,
            actor_id,
            actor_is_bot: false,
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: title.to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
        },
    )
    .await?;
    Ok(accepted.object.document_id)
}

#[derive(FromQueryResult)]
struct BytesValue {
    value: Vec<u8>,
}

async fn add_retained_update(
    db: &DatabaseConnection,
    document_id: Uuid,
    workspace_id: Uuid,
    actor_id: Uuid,
) -> anyhow::Result<()> {
    let snapshot = BytesValue::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT snapshot AS value FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("fixture document is missing"))?
    .value;
    let mut engine = LoroCollabEngine::load(&snapshot)?;
    let frontier = engine.frontier();
    engine.set_title("Backup fixture one updated")?;
    let bytes = engine.export_from(&frontier)?;
    let epoch = authz::read_epoch(db, workspace_id).await?;
    let outcome = accept_update(
        db,
        &WarmCache::new(),
        &DocumentCoordinator::new(),
        &SessionRegistry::new(),
        &SnapshotAdvancer::new(),
        10,
        None,
        UpdateRequest {
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
            document_id,
            update_id: Uuid::new_v4(),
            bytes,
            idempotency_key: None,
            event_idempotency_key: None,
            origin_client_id: Some("backup-source-fixture".to_string()),
            message: None,
            actor_id,
            actor_is_bot: false,
            workspace_id,
            checked_epoch: epoch,
            expected_frontier: None,
        },
    )
    .await?;
    anyhow::ensure!(
        matches!(outcome, AcceptOutcome::Accepted(_)),
        "fixture update was not accepted"
    );
    Ok(())
}

#[derive(FromQueryResult)]
struct I64Value {
    value: i64,
}

async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> anyhow::Result<i64> {
    Ok(
        I64Value::find_by_statement(Statement::from_string(DbBackend::Postgres, sql.to_string()))
            .one(db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("scalar query returned no row"))?
            .value,
    )
}
