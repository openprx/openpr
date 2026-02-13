use clap::Parser;
use platform::{
    app::connect_db,
    config::AppConfig,
    logging,
};

#[derive(Debug, Parser)]
struct WorkerArgs {
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let cfg = AppConfig::from_env("worker", "0.0.0.0:8081")?;
    logging::init("worker");

    let db = connect_db(&cfg.database_url).await?;
    tracing::info!(concurrency = args.concurrency, app = %cfg.app_name, "worker started");

    loop {
        if let Err(err) = db.ping().await {
            tracing::warn!(error = %err, "worker ping failed");
        }
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}
