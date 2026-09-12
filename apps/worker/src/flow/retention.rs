//! Worker-owned irreversible Flow object cleanup.
//!
//! Only rows explicitly marked by the full-access archive path are eligible. An ordinary
//! edit-tier Page archive has `permanent_cleanup_after IS NULL` and is therefore invisible to
//! this job.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

#[derive(Debug, FromQueryResult)]
struct Candidate {
    id: Uuid,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    pub selected: u64,
    pub deleted: u64,
    pub raced: u64,
}

pub async fn run_tick(db: &DatabaseConnection, requested_batch_size: usize) -> anyhow::Result<TickReport> {
    run_tick_at(db, requested_batch_size, chrono::Utc::now()).await
}

async fn run_tick_at(
    db: &DatabaseConnection,
    requested_batch_size: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<TickReport> {
    let limit = i64::try_from(requested_batch_size.clamp(1, 100)).unwrap_or(100);
    let tx = db.begin().await?;
    let candidates = Candidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects \
          WHERE lifecycle_status='archived' AND permanent_cleanup_after < $2 \
          ORDER BY permanent_cleanup_after,id FOR UPDATE SKIP LOCKED LIMIT $1",
        vec![limit.into(), now.into()],
    ))
    .all(&tx)
    .await?;
    let mut report = TickReport {
        selected: candidates.len() as u64,
        ..TickReport::default()
    };
    for candidate in candidates {
        let deleted = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_objects WHERE id=$1 AND lifecycle_status='archived' \
                   AND permanent_cleanup_after < $2",
                vec![candidate.id.into(), now.into()],
            ))
            .await?
            .rows_affected();
        report.deleted += deleted;
        if deleted == 0 {
            report.raced += 1;
        }
    }
    tx.commit().await?;
    Ok(report)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::print_stderr, clippy::unwrap_used)]
mod tests {
    use api::flow::command::{CreateObjectInput, ExecuteCommandInput, create_object, execute_command};
    use api::flow::event_origin::{CommandOrigin, EventSurface};
    use platform::{
        app::{AppState, FlowPermissionCacheSlot},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use serde_json::json;
    use uuid::Uuid;

    use super::run_tick_at;

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
            if let Ok(admin) = Database::connect(&admin_url).await {
                let _ = admin
                    .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                    .await;
            }
        }
    }

    async fn scratch() -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url).await.expect("test database connects");
        let name = "openpr_worker_v08_object_retention".to_string();
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await
            .expect("old scratch drops");
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("scratch creates");
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let db = Database::connect(format!("{prefix}/{name}"))
            .await
            .expect("scratch connects");
        let mut migrations = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"))
            .expect("migration directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect::<Vec<_>>();
        migrations.sort();
        for path in migrations {
            db.execute_unprepared(&std::fs::read_to_string(&path).expect("migration reads"))
                .await
                .unwrap_or_else(|error| panic!("{} failed: {error}", path.display()));
        }
        Some(Scratch { db, name, admin_url })
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .expect("fixture statement succeeds");
    }

    #[tokio::test]
    async fn worker_deletes_only_expired_full_access_archives_not_edit_tier_soft_archives() {
        let Some(scratch) = scratch().await else {
            eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
            return;
        };
        let owner = Uuid::new_v4();
        let member = Uuid::new_v4();
        let workspace = Uuid::new_v4();
        for user in [owner, member] {
            exec(
                &scratch.db,
                "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','test','user',true)",
                vec![user.into(), format!("{user}@retention.test").into()],
            )
            .await;
        }
        exec(
            &scratch.db,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'retention test',$3)",
            vec![workspace.into(), format!("retention-{workspace}").into(), owner.into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner'),($1,$3,'member')",
            vec![workspace.into(), owner.into(), member.into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled,default_member_level) VALUES($1,true,'edit')",
            vec![workspace.into()],
        )
        .await;
        let navigator: Uuid = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM flow_objects WHERE workspace_id=$1 AND object_type='navigator'",
                vec![workspace.into()],
            ))
            .await
            .expect("navigator query")
            .expect("navigator exists")
            .try_get("", "id")
            .expect("navigator id");
        let state = AppState {
            cfg: AppConfig {
                app_name: "retention-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("retention-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db: scratch.db.clone(),
            flow_permission_cache: FlowPermissionCacheSlot::default(),
        };
        let create = |object_type: &str, title: &str| CreateObjectInput {
            workspace_id: workspace,
            actor_id: owner,
            actor_is_bot: false,
            object_type: object_type.to_string(),
            project_id: None,
            parent_object_id: Some(navigator),
            title: title.to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        };
        let page = create_object(&state, create("page", "soft page"))
            .await
            .expect("page creates")
            .object
            .id;
        let collection = create_object(&state, create("collection", "retained collection"))
            .await
            .expect("collection creates")
            .object
            .id;
        let archive = |object_id: Uuid, actor_id: Uuid, role: &str| ExecuteCommandInput {
            object_id,
            actor_id,
            principal_kind: "user".to_string(),
            role: role.to_string(),
            command_type: "archive".to_string(),
            payload: json!({}),
            expected_frontier: None,
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
            origin_client_id: "retention-test".to_string(),
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        };
        execute_command(&state, archive(page, member, "member"))
            .await
            .expect("edit member soft-archives page");
        execute_command(&state, archive(collection, owner, "owner"))
            .await
            .expect("owner archives collection at full-access tier");

        let report = run_tick_at(&scratch.db, 10, chrono::Utc::now() + chrono::Duration::days(31))
            .await
            .expect("retention tick succeeds");
        assert_eq!(report.deleted, 1);
        let survivors = scratch
            .db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id,permanent_cleanup_after FROM flow_objects WHERE id=ANY($1)",
                vec![vec![page, collection].into()],
            ))
            .await
            .expect("survivors query");
        assert_eq!(survivors.len(), 1);
        let survivor = survivors.first().expect("one survivor");
        assert_eq!(survivor.try_get::<Uuid>("", "id").expect("survivor id"), page);
        assert_eq!(
            survivor
                .try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "permanent_cleanup_after")
                .expect("cleanup deadline"),
            None
        );

        scratch.drop_self().await;
    }
}
