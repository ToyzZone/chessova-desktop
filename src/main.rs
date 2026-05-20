mod config;
mod engine;
mod engine_status;
mod orchestrator;
mod protocol;
mod ws_server;

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

    // Tokio runtime + WebSocket server on a background thread so the
    // main thread is free to run the OS event loop (Cocoa / Win32 both
    // require their event loops on the main thread).
    let config_for_ws = config.clone();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                warn!(err = %e, "failed to build tokio runtime");
                return;
            }
        };
        if let Err(e) = runtime.block_on(ws_server::serve(config_for_ws)) {
            warn!(err = %e, "ws server exited with error");
        }
    });

    // Main thread: tray icon event loop. Blocks until Quit is selected.
    // Quit calls std::process::exit, which terminates the whole process
    // including the background WS thread.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if let Err(e) = tray::run(status) {
            warn!(err = %e, "tray icon failed to start; running headless");
            std::thread::park();
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux / other: no tray icon yet, park the main thread forever.
        std::thread::park();
    }

    Ok(())
}
