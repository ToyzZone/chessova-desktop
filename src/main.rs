mod config;
mod engine;
mod engine_status;
mod install_paths;
mod installer;
mod lc0_weights;
mod orchestrator;
mod protocol;
mod ws_server;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod progress_window;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray;

use std::sync::Arc;
use tracing::{info, warn};

use crate::engine_status::EngineStatus;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chessova_desktop=info".into()),
        )
        .init();

    let config = Arc::new(config::Config::from_env());
    let status = EngineStatus::probe(&config);

    info!(
        stockfish = %config.stockfish_path,
        lc0 = ?config.lc0_path,
        lc0_weights = ?config.lc0_weights,
        threads = config.sf_threads,
        hash_mb = config.sf_hash_mb,
        batch_workers = config.sf_batch_workers,
        addr = %config.bind_addr,
        health = ?status.health(),
        "Chessova Desktop helper starting"
    );

    // Build the tokio runtime explicitly so we can hand its Handle to
    // the tray module — install jobs are tokio tasks but the tray
    // event loop owns the main thread.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("tokio runtime build failed: {}", e))?;
    let handle = runtime.handle().clone();

    // WebSocket server on a background tokio task.
    let config_for_ws = config.clone();
    runtime.spawn(async move {
        if let Err(e) = ws_server::serve(config_for_ws).await {
            warn!(err = %e, "ws server exited with error");
        }
    });

    // Keep the runtime alive on a dedicated thread (its destructor
    // would otherwise shut down tasks when `runtime` is dropped).
    std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    // Main thread: tray icon event loop. Blocks until Quit selected.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if let Err(e) = tray::run(status, handle) {
            warn!(err = %e, "tray icon failed to start; running headless");
            std::thread::park();
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = handle;
        std::thread::park();
    }

    Ok(())
}
