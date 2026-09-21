//! 摸金小王子 · mj-server
//! 家庭自用 NAS 后端骨架（PRD V3.2）

mod ai;
mod backtest;
mod db;
mod error;
mod export;
mod market;
mod paper;
mod perf;
mod research;
mod routes;
mod signals;
mod universe;
mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::db::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mj_server=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_path = std::env::var("MJ_DB_PATH").unwrap_or_else(|_| "data/mojin.db".into());
    let host = std::env::var("MJ_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("MJ_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8787);

    let path = PathBuf::from(&db_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let state = Arc::new(AppState::open(&path)?);
    state.migrate()?;
    tracing::info!(db = %path.display(), "sqlite ready");

    // 启动后异步回填历史信号后验（不阻塞监听）
    let state_for_backfill = state.clone();
    tokio::spawn(async move {
        // 错峰几秒，让 server 先就绪
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        match perf::backfill_all(&state_for_backfill, 500).await {
            Ok((total, updated, skipped)) => {
                tracing::info!(total, updated, skipped, "startup signal backfill done");
            }
            Err(e) => tracing::warn!(err=%e, "startup signal backfill err"),
        }
    });

    let app = Router::new()
        .merge(routes::router())
        .route("/ws", axum::routing::get(ws::ws_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    // 启动 30s 周期广播 equity + healthz
    {
        let s = state.clone();
        tokio::spawn(async move { ws::periodic_broadcast(s).await });
    }

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!(%addr, "mj-server listening · 家庭自用 · 不构成投资建议");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown");
}
