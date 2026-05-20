use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, info};

use crate::{
    config::Config,
    engine::Engine,
    protocol::{BatchResult, ClientMessage, EngineId, HelperEngineInfo, HelperMessage},
};

const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Per-connection state shared across messages.
///
/// Persistent engine processes: one Stockfish + one Lc0 per connection,
/// reused across every live `Analyze` call. The cold start cost (especially
/// Lc0 loading its network into memory) is paid once on first use instead
/// of on every navigation.
///
/// In-flight tracking: a new live `Analyze` for an engine implicitly preempts
/// whatever that engine was doing — the previous stop signal fires, the engine
/// drains to `bestmove`, then the new request acquires the engine mutex.
/// External `Stop { id }` walks the same per-engine stoppers via the
/// `request_engines` reverse lookup.
///
/// Batch analyze does NOT share the live engine pool: it spawns its own
/// short-lived workers so batch work never blocks navigation.
#[derive(Default)]
pub struct State {
    /// Persistent engine processes shared across live analyze calls.
    engines: Mutex<HashMap<EngineId, Arc<Mutex<Engine>>>>,
    /// Per-engine: the stop sender for the in-flight live analyze on that
    /// engine. A new analyze takes-and-fires to preempt the previous one.
    engine_stoppers: Mutex<HashMap<EngineId, oneshot::Sender<()>>>,
    /// request_id -> engines used by that request. External `Stop { id }`
    /// uses this to look up which engine_stoppers to fire.
    request_engines: Mutex<HashMap<String, Vec<EngineId>>>,
}

impl State {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Get an existing persistent engine, or spawn it on first use.
    async fn get_or_spawn(
        &self,
        engine_id: EngineId,
        path: &str,
        options: &[(String, String)],
    ) -> anyhow::Result<Arc<Mutex<Engine>>> {
        let mut engines = self.engines.lock().await;
        if let Some(e) = engines.get(&engine_id) {
            return Ok(e.clone());
        }
        let engine = Engine::spawn(path, engine_id.clone(), options).await?;
        let arc = Arc::new(Mutex::new(engine));
        engines.insert(engine_id, arc.clone());
        Ok(arc)
    }

    /// Take and fire the in-flight stop sender for this engine, if any.
    /// Called both by a new live Analyze (auto-preempt) and by external Stop.
    async fn preempt_engine(&self, engine_id: &EngineId) {
        let mut stoppers = self.engine_stoppers.lock().await;
        if let Some(tx) = stoppers.remove(engine_id) {
            let _ = tx.send(());
        }
    }

    async fn install_engine_stopper(
        &self,
        engine_id: EngineId,
        tx: oneshot::Sender<()>,
    ) {
        self.engine_stoppers
            .lock()
            .await
            .insert(engine_id, tx);
    }

    /// Drop the engine_stopper entry for this engine if it still points at us.
    /// Used after analyze completes normally so a later Stop doesn't fire
    /// against a stale sender.
    async fn clear_engine_stopper(&self, engine_id: &EngineId) {
        self.engine_stoppers.lock().await.remove(engine_id);
    }

    async fn register_request(&self, request_id: String, engine_ids: Vec<EngineId>) {
        self.request_engines
            .lock()
            .await
            .insert(request_id, engine_ids);
    }

    async fn forget_request(&self, request_id: &str) {
        self.request_engines.lock().await.remove(request_id);
    }

    async fn stop_request(&self, request_id: &str) {
        let engine_ids = self
            .request_engines
            .lock()
            .await
            .remove(request_id)
            .unwrap_or_default();
        for e in engine_ids {
            self.preempt_engine(&e).await;
        }
    }
}

/// Build the hello-ack payload from the current config.
pub fn make_hello_ack(config: &Config) -> HelperMessage {
    let mut engines = vec![HelperEngineInfo {
        id: "stockfish".into(),
        name: "Stockfish".into(),
        version: None,
        threads: Some(config.sf_threads),
        hash_mb: Some(config.sf_hash_mb),
        batch_workers: Some(config.sf_batch_workers),
    }];
    if config.lc0_path.is_some() {
        engines.push(HelperEngineInfo {
            id: "lc0".into(),
            name: "Lc0".into(),
            version: None,
            // Lc0 sets Threads=auto internally; not surfaced here.
            threads: None,
            hash_mb: None,
            batch_workers: None,
        });
    }
    HelperMessage::HelloAck {
        version: HELPER_VERSION.into(),
        engines,
    }
}

/// Handle one client message; sends zero or more HelperMessages back via `tx`.
pub async fn handle(
    msg: ClientMessage,
    config: &Config,
    state: &Arc<State>,
    tx: &mpsc::Sender<HelperMessage>,
) {
    match msg {
        ClientMessage::Hello { .. } => {
            let ack = make_hello_ack(config);
            let _ = tx.send(ack).await;
        }

        ClientMessage::Analyze {
            id,
            fen,
            engines,
            multi_pv,
            depth,
            stream,
            threads,
            hash_mb,
        } => {
            let mut handles = Vec::new();
            let mut request_engine_ids: Vec<EngineId> = Vec::new();

            for engine_id in engines {
                let path = match engine_path(engine_id.clone(), config) {
                    Some(p) => p,
                    None => {
                        let _ = tx
                            .send(HelperMessage::Error {
                                id: Some(id.clone()),
                                code: "engine-not-found".into(),
                                message: format!("{:?} is not configured", engine_id),
                            })
                            .await;
                        continue;
                    }
                };
                let options = uci_options_for(&engine_id, config);

                // Auto-preempt any in-flight live analyze on this engine.
                // The engine will drain to bestmove, then release the mutex
                // so the new request below can take it.
                state.preempt_engine(&engine_id).await;

                let engine_arc = match state
                    .get_or_spawn(engine_id.clone(), &path, &options)
                    .await
                {
                    Ok(e) => e,
                    Err(e) => {
                        error!(err = %e, "engine spawn error");
                        let _ = tx
                            .send(HelperMessage::Error {
                                id: Some(id.clone()),
                                code: "engine-spawn".into(),
                                message: e.to_string(),
                            })
                            .await;
                        continue;
                    }
                };

                let (stop_tx, stop_rx) = oneshot::channel::<()>();
                state
                    .install_engine_stopper(engine_id.clone(), stop_tx)
                    .await;
                request_engine_ids.push(engine_id.clone());

                let id2 = id.clone();
                let fen2 = fen.clone();
                let tx2 = tx.clone();
                let state2 = state.clone();
                let engine_id2 = engine_id.clone();

                handles.push(tokio::spawn(async move {
                    let mut eng = engine_arc.lock().await;
                    let result = eng
                        .analyze(
                            &id2,
                            &fen2,
                            multi_pv,
                            depth,
                            stream,
                            threads,
                            hash_mb,
                            tx2.clone(),
                            Some(stop_rx),
                        )
                        .await;
                    // Whether analyze finished normally or via stop, the
                    // stopper for this engine is now consumed/stale.
                    state2.clear_engine_stopper(&engine_id2).await;
                    if let Err(e) = result {
                        error!(err = %e, "analyze error");
                        let _ = tx2
                            .send(HelperMessage::Error {
                                id: Some(id2),
                                code: "engine-error".into(),
                                message: e.to_string(),
                            })
                            .await;
                    }
                }));
            }

            state
                .register_request(id.clone(), request_engine_ids)
                .await;

            for h in handles {
                let _ = h.await;
            }

            state.forget_request(&id).await;
        }

        ClientMessage::AnalyzeBatch {
            id,
            fens,
            engines,
            multi_pv,
            depth,
            threads,
            hash_mb,
            workers,
        } => {
            let total = fens.len() as u32;
            // Worker count comes from client override (settings modal /
            // preset) or falls back to helper auto (config.sf_batch_workers).
            // Clamp to [1, ncpu] so a runaway client value can't blow up
            // the machine.
            let ncpu = std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(2);
            let requested_workers = workers.unwrap_or(config.sf_batch_workers);
            let num_workers: usize = requested_workers.clamp(1, ncpu).max(1) as usize;

            let mut all_results: Vec<BatchResult> = Vec::new();

            for engine_id in &engines {
                let path = match engine_path(engine_id.clone(), config) {
                    Some(p) => p,
                    None => {
                        let _ = tx
                            .send(HelperMessage::Error {
                                id: Some(id.clone()),
                                code: "engine-not-found".into(),
                                message: format!("{:?} is not configured", engine_id),
                            })
                            .await;
                        continue;
                    }
                };
                // Spawn options divide the user's thread/hash budget
                // across the parallel workers. With 8 cores + 4 workers
                // each SF gets 2 threads × 128 MB hash = 8 threads, 512
                // MB total. Same shape as 1 × 8 × 512 MB but parallel.
                //
                // The per-position override inside engine.analyze() is
                // also divided so mid-batch settings changes track the
                // same per-worker math.
                let total_threads = threads.unwrap_or(config.sf_threads);
                let total_hash = hash_mb.unwrap_or(config.sf_hash_mb);
                let per_worker_threads =
                    (total_threads / num_workers as u32).max(1);
                let per_worker_hash =
                    (total_hash / num_workers as u32).max(64);
                let options = batch_uci_options_for(
                    engine_id,
                    config,
                    per_worker_threads,
                    per_worker_hash,
                );

                // Contiguous chunking — adjacent positions share Stockfish's TT.
                let chunk_size = fens.len().div_ceil(num_workers).max(1);
                let chunks: Vec<Vec<(usize, String)>> = fens
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (i, f.clone()))
                    .collect::<Vec<_>>()
                    .chunks(chunk_size)
                    .map(|c| c.to_vec())
                    .collect();

                let (result_tx, mut result_rx) =
                    mpsc::channel::<(usize, BatchResult)>(num_workers * 4);

                for chunk in chunks {
                    if chunk.is_empty() {
                        continue;
                    }
                    let path2 = path.clone();
                    let engine_id2 = engine_id.clone();
                    let options2 = options.clone();
                    let result_tx2 = result_tx.clone();

                    tokio::spawn(async move {
                        let mut engine =
                            match Engine::spawn(&path2, engine_id2.clone(), &options2).await {
                                Ok(e) => e,
                                Err(e) => {
                                    error!(err = %e, "batch worker spawn error");
                                    return;
                                }
                            };

                        for (idx, fen) in chunk {
                            let (inner_tx, mut inner_rx) =
                                mpsc::channel::<HelperMessage>(16);
                            let req_id = format!("batch_inner_{}", idx);
                            // Pass the per-worker thread/hash split,
                            // not the user's total budget — see the
                            // comment above num_workers for why.
                            if let Err(e) = engine
                                .analyze(
                                    &req_id,
                                    &fen,
                                    multi_pv,
                                    depth,
                                    false,
                                    Some(per_worker_threads),
                                    Some(per_worker_hash),
                                    inner_tx,
                                    None,
                                )
                                .await
                            {
                                error!(err = %e, "batch analyze error");
                                continue;
                            }
                            while let Some(msg) = inner_rx.recv().await {
                                if let HelperMessage::AnalyzeDone {
                                    lines,
                                    depth: done_depth,
                                    ..
                                } = msg
                                {
                                    let _ = result_tx2
                                        .send((
                                            idx,
                                            BatchResult {
                                                fen: fen.clone(),
                                                engine: engine_id2.clone(),
                                                lines,
                                                depth: done_depth,
                                            },
                                        ))
                                        .await;
                                    break;
                                }
                            }
                        }
                    });
                }

                drop(result_tx);

                let mut engine_results: Vec<Option<BatchResult>> =
                    (0..fens.len()).map(|_| None).collect();
                let mut completed: u32 = 0;
                while let Some((idx, br)) = result_rx.recv().await {
                    if idx < engine_results.len() {
                        engine_results[idx] = Some(br);
                    }
                    completed += 1;
                    let _ = tx
                        .send(HelperMessage::AnalyzeBatchProgress {
                            id: id.clone(),
                            completed,
                            total,
                        })
                        .await;
                }

                for r in engine_results.into_iter().flatten() {
                    all_results.push(r);
                }
            }

            let _ = tx
                .send(HelperMessage::AnalyzeBatchDone {
                    id,
                    results: all_results,
                })
                .await;
        }

        ClientMessage::Stop { id } => {
            info!(id = %id, "stop requested");
            state.stop_request(&id).await;
        }
    }
}

fn engine_path(id: EngineId, config: &Config) -> Option<String> {
    match id {
        EngineId::Stockfish => Some(config.stockfish_path.clone()),
        EngineId::Lc0 => config.lc0_path.clone(),
    }
}

fn uci_options_for(id: &EngineId, config: &Config) -> Vec<(String, String)> {
    match id {
        EngineId::Lc0 => {
            let mut out = Vec::new();
            if let Some(weights) = &config.lc0_weights {
                out.push(("WeightsFile".to_string(), weights.clone()));
            }
            out
        }
        EngineId::Stockfish => vec![
            ("Threads".into(), config.sf_threads.to_string()),
            ("Hash".into(), config.sf_hash_mb.to_string()),
        ],
    }
}

/// Batch variant: every worker spawns its own SF, so the caller computes
/// `threads_per_worker = ncpu / num_workers` and `hash_per_worker =
/// sf_hash_mb / num_workers` to keep the totals at host capacity.
fn batch_uci_options_for(
    id: &EngineId,
    config: &Config,
    threads_per_worker: u32,
    hash_per_worker: u32,
) -> Vec<(String, String)> {
    match id {
        EngineId::Lc0 => uci_options_for(id, config),
        EngineId::Stockfish => vec![
            ("Threads".into(), threads_per_worker.to_string()),
            ("Hash".into(), hash_per_worker.to_string()),
        ],
    }
}

