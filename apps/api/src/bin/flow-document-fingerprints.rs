//! Emits stable JSON fingerprints for every Flow document in one database.

#![allow(clippy::print_stdout)]

use api::flow::collab::integrity::all_document_fingerprints;
use sea_orm::Database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let database_url =
        std::env::var("OPENPR_DATABASE_URL").map_err(|_| anyhow::anyhow!("OPENPR_DATABASE_URL is required"))?;
    let db = Database::connect(database_url).await?;
    let fingerprints = all_document_fingerprints(&db)
        .await
        .map_err(|error| anyhow::anyhow!("document fingerprint verification failed: {error}"))?;
    println!("{}", serde_json::to_string(&fingerprints)?);
    db.close().await?;
    Ok(())
}
