//! Persistent v0.8 application-rollback interlocks.
//!
//! Operators set these flags before starting a v0.7 application against a v0.8 database. The
//! old application ignores the forward-compatible table, while every v0.8 writer covered by the
//! rollback contract fails closed when its flag is set or the row cannot be read.

use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};

#[derive(Debug, Clone, Copy, FromQueryResult)]
pub struct RollbackControl {
    pub compaction_paused: bool,
    pub retention_paused: bool,
    pub import_promotion_paused: bool,
}

pub async fn control(db: &DatabaseConnection) -> Result<RollbackControl, sea_orm::DbErr> {
    RollbackControl::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT compaction_paused,retention_paused,import_promotion_paused \
           FROM flow_v08_rollback_control WHERE singleton=true"
            .to_string(),
    ))
    .one(db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::RecordNotFound("flow_v08_rollback_control singleton".to_string()))
}
