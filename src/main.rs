mod config;
mod engine;
mod orchestrator;
mod protocol;
mod ws_server;

use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chessova_desktop=info".into()),
        )
        .init();

    let config = Arc::new(config::Config::from_env());
    info!(
        stockfish = %config.stockfish_path,
        lc0 = ?config.lc0_path,
        lc0_weights = ?config.lc0_weights,
        addr = %config.bind_addr,
        "Chessova Desktop helper (headless) starting"
    );

    ws_server::serve(config).await?;
    Ok(())
}
