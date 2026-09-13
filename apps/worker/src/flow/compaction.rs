//! Worker-owned v0.8 compaction scheduler. Hot collaboration never enters this module: it scans
//! persisted document facts and invokes the same exact-head compaction service as admin surfaces.

use api::flow::collab::{compaction, limits, snapshot};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

const MAX_BATCH_SIZE: usize = 100;

#[derive(Debug, FromQueryResult)]
struct CandidateRow {
    document_id: Uuid,
    head_seq: i64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    pub examined: u64,
    pub compacted: u64,
    pub retained_for_acks: u64,
    pub failed: u64,
}

/// Scans a bounded set selected by real persisted update count/bytes, then re-evaluates the same
/// production thresholds immediately before compaction. The scheduler never invents a separate
/// threshold and never force-resyncs a client; only an explicit admin execution may do that.
pub async fn run_tick(db: &DatabaseConnection, requested_batch_size: usize) -> anyhow::Result<TickReport> {
    run_tick_with_thresholds(
        db,
        requested_batch_size,
        limits::SNAPSHOT_TAIL_UPDATES_SOFT_MAX,
        limits::SNAPSHOT_TAIL_BYTES_SOFT_MAX,
    )
    .await
}

async fn run_tick_with_thresholds(
    db: &DatabaseConnection,
    requested_batch_size: usize,
    tail_updates_soft_max: i64,
    tail_bytes_soft_max: i64,
) -> anyhow::Result<TickReport> {
    if api::flow::rollback::control(db).await?.compaction_paused {
        return Ok(TickReport::default());
    }
    let limit = requested_batch_size.clamp(1, MAX_BATCH_SIZE);
    let rows = CandidateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT cd.id AS document_id, cd.head_seq \
           FROM collab_documents cd \
          WHERE cd.head_seq > cd.compaction_boundary_seq \
            AND (cd.head_seq - cd.snapshot_seq >= $1 OR \
                 COALESCE((SELECT sum(octet_length(cu.bytes)) FROM collab_updates cu \
                            WHERE cu.document_id = cd.id AND cu.seq > cd.snapshot_seq), 0) >= $2) \
          ORDER BY cd.updated_at ASC, cd.id ASC LIMIT $3",
        vec![
            tail_updates_soft_max.into(),
            tail_bytes_soft_max.into(),
            i64::try_from(limit).unwrap_or(i64::MAX).into(),
        ],
    ))
    .all(db)
    .await?;

    let mut report = TickReport::default();
    for row in rows {
        report.examined += 1;
        let Some(stats) = snapshot::read_tail_stats(db, row.document_id).await? else {
            report.failed += 1;
            continue;
        };
        if stats.tail_updates < tail_updates_soft_max && stats.tail_bytes < tail_bytes_soft_max {
            continue;
        }
        match compaction::compact(db, row.document_id, row.head_seq, false).await {
            Ok(result) => {
                report.compacted += 1;
                if result.disposition == compaction::HistoryDisposition::RetainForLaggingClients {
                    report.retained_for_acks += 1;
                }
            }
            Err(error) => {
                report.failed += 1;
                tracing::warn!(document_id = %row.document_id, %error, "flow compaction tick failed");
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::print_stderr)]
mod tests {
    use api::flow::collab::integrity::document_fingerprint;
    use api::flow::command::{CreateObjectInput, ExecuteCommandInput, create_object, execute_command};
    use api::flow::event_origin::{CommandOrigin, EventSurface};
    use platform::{
        app::{AppState, FlowPermissionCacheSlot},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use uuid::Uuid;

    use super::{TickReport, run_tick_with_thresholds};

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
        let name = "openpr_worker_v08_compaction".to_string();
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

    fn state(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-compaction-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-compaction-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: FlowPermissionCacheSlot::default(),
        }
    }

    #[tokio::test]
    async fn flow_compaction_worker_preserves_exact_document_fingerprint() {
        let Some(scratch) = scratch().await else {
            eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
            return;
        };
        let owner = Uuid::new_v4();
        let workspace = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','test','user',true)",
            vec![owner.into(), format!("{owner}@compaction.test").into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'compaction test',$3)",
            vec![workspace.into(), format!("compaction-{workspace}").into(), owner.into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
            vec![workspace.into(), owner.into()],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled,default_member_level) VALUES($1,true,'edit')",
            vec![workspace.into()],
        )
        .await;
        let state = state(scratch.db.clone());
        let created = create_object(
            &state,
            CreateObjectInput {
                workspace_id: workspace,
                actor_id: owner,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "before compaction".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("page creates");
        execute_command(
            &state,
            ExecuteCommandInput {
                object_id: created.object.id,
                actor_id: owner,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "set_title".to_string(),
                payload: serde_json::json!({"title": "after compaction"}),
                expected_frontier: None,
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin_client_id: "worker-compaction-test".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("accepted update creates a real compaction tail");

        let before = document_fingerprint(&scratch.db, created.object.document_id)
            .await
            .expect("pre-compaction fingerprint");
        exec(
            &scratch.db,
            "UPDATE flow_v08_rollback_control SET compaction_paused=true,reason='v0.7 rollback test',changed_at=now() WHERE singleton=true",
            vec![],
        )
        .await;
        let paused = run_tick_with_thresholds(&scratch.db, 1, 1, i64::MAX)
            .await
            .expect("paused compaction tick succeeds without work");
        assert_eq!(paused, TickReport::default());
        let retained_while_paused: i64 = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*)::bigint AS count FROM collab_updates WHERE document_id=$1",
                vec![created.object.document_id.into()],
            ))
            .await
            .expect("paused tail count query")
            .expect("paused tail count row")
            .try_get("", "count")
            .expect("paused tail count");
        assert_eq!(
            retained_while_paused, 1,
            "rollback pause must retain the real compaction candidate"
        );
        exec(
            &scratch.db,
            "UPDATE flow_v08_rollback_control SET compaction_paused=false,reason=NULL,changed_at=now() WHERE singleton=true",
            vec![],
        )
        .await;
        let report = run_tick_with_thresholds(&scratch.db, 1, 1, i64::MAX)
            .await
            .expect("worker compaction tick succeeds");
        assert_eq!(report.examined, 1);
        assert_eq!(report.compacted, 1);
        let after = document_fingerprint(&scratch.db, created.object.document_id)
            .await
            .expect("post-compaction fingerprint");
        assert_eq!(after.head_seq, before.head_seq);
        assert_eq!(after.head_frontier, before.head_frontier);
        assert_eq!(after.semantic_hash, before.semantic_hash);
        assert_eq!(after.projection_seq, before.projection_seq);
        let remaining: i64 = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*)::bigint AS count FROM collab_updates WHERE document_id=$1",
                vec![created.object.document_id.into()],
            ))
            .await
            .expect("tail count query")
            .expect("tail count row")
            .try_get("", "count")
            .expect("tail count");
        assert_eq!(remaining, 0, "the test must prove compaction actually happened");
        scratch.drop_self().await;
    }
}
