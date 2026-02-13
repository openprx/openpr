use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use platform::{
    app::{AppState, connect_db},
    config::AppConfig,
    logging,
};
use serde::Serialize;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::from_env("api", "0.0.0.0:8080")?;
    logging::init("api");

    let db = connect_db(&cfg.database_url).await?;
    let state = AppState {
        cfg: cfg.clone(),
        db,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .layer(CompressionLayer::new())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    tracing::info!(bind_addr = %cfg.bind_addr, "api server started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let body = HealthResponse {
        status: "ok",
        service: state.cfg.app_name,
    };
    (StatusCode::OK, Json(body))
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.ping().await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"status":"ready"}))).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "database not ready");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"status":"not_ready"})),
            )
                .into_response()
        }
    }
}
