mod api;
mod config;
mod db;
mod error;
mod google;

use std::sync::Arc;

use anyhow::Context;
use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rterm_server=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env().context("invalid configuration")?;
    let pool = db::connect(&cfg.database_url).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let bind = cfg.bind.clone();
    let state = api::AppState {
        cfg: Arc::new(cfg),
        db: pool,
        http,
    };
    let app = api::router(state);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("cannot bind {bind}"))?;
    tracing::info!("rterm sync server listening on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
