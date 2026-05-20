/// Wire protocol types matching lib/review/uciProtocol.ts and lib/review/types.ts exactly.
///
/// Serialization rules:
///   - `type` discriminant tag uses kebab-case  (serde tag + rename_all on the enum)
///   - struct fields use camelCase              (serde rename_all on each struct / inline type)
///
/// Example round-trip for analyze-progress (client receives):
/// {
///   "type": "analyze-progress",
///   "id": "req_lq5k2_ab3def",
///   "engine": "stockfish",
///   "fen": "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
///   "depth": 18,
///   "lines": [
///     { "pv": ["e7e5", "g1f3"], "evalCp": 30, "evalMate": null, "depth": 18 },
///     { "pv": ["c7c5", "g1f3"], "evalCp": 22, "evalMate": null, "depth": 18 }
///   ]
/// }
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared leaf types
// ---------------------------------------------------------------------------

/// Matches lib/review/types.ts EngineLine
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineLine {
    /// UCI move tokens, e.g. ["e2e4", "e7e5"]
    pub pv: Vec<String>,
    pub eval_cp: Option<i32>,
    pub eval_mate: Option<i32>,
    pub depth: u32,
}

/// Matches lib/review/types.ts HelperEngineInfo
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperEngineInfo {
    /// "stockfish" | "lc0"
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    /// Threads configured on this engine. Surfaced so the UI can show
    /// users why the helper is so much faster than the WASM server path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Hash table size in MB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_mb: Option<u32>,
}

/// TS: EngineId = "stockfish" | "lc0"
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EngineId {
    Stockfish,
    Lc0,
}

// ---------------------------------------------------------------------------
// ClientMessage  (messages arriving FROM the browser)
// ---------------------------------------------------------------------------

/// Matches ClientMessage in lib/review/uciProtocol.ts
///
/// Example hello:
/// { "type": "hello", "clientVersion": "1.0.0" }
///
/// Example analyze:
/// {
///   "type": "analyze",
///   "id": "req_lq5k2_ab3def",
///   "fen": "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
///   "engines": ["stockfish"],
///   "multiPv": 3,
///   "depth": 20,
///   "stream": true
/// }
///
/// Example analyze-batch:
/// {
///   "type": "analyze-batch",
///   "id": "req_lq5k2_ab3def",
///   "fens": ["<fen1>", "<fen2>"],
///   "engines": ["stockfish", "lc0"],
///   "multiPv": 3,
///   "depth": 20
/// }
///
/// Example stop:
/// { "type": "stop", "id": "req_lq5k2_ab3def" }
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    Hello {
        #[serde(rename = "clientVersion")]
        client_version: String,
    },
    Analyze {
        id: String,
        fen: String,
        engines: Vec<EngineId>,
        #[serde(rename = "multiPv")]
        multi_pv: u32,
        depth: u32,
        stream: bool,
    },
    AnalyzeBatch {
        id: String,
        fens: Vec<String>,
        engines: Vec<EngineId>,
        #[serde(rename = "multiPv")]
        multi_pv: u32,
        depth: u32,
    },
    Stop {
        id: String,
    },
}

// ---------------------------------------------------------------------------
// HelperMessage  (messages sent TO the browser)
// ---------------------------------------------------------------------------

/// Matches HelperMessage in lib/review/uciProtocol.ts
///
/// Example hello-ack (server → client):
/// {
///   "type": "hello-ack",
///   "version": "0.1.0",
///   "engines": [
///     { "id": "stockfish", "name": "Stockfish 16", "version": "16" },
///     { "id": "lc0",       "name": "Lc0",          "version": null }
///   ]
/// }
///
/// Example analyze-progress (server → client):
/// {
///   "type": "analyze-progress",
///   "id": "req_lq5k2_ab3def",
///   "engine": "stockfish",
///   "fen": "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
///   "depth": 18,
///   "lines": [{ "pv": ["e7e5"], "evalCp": 30, "evalMate": null, "depth": 18 }]
/// }
///
/// Example analyze-done (server → client):
/// {
///   "type": "analyze-done",
///   "id": "req_lq5k2_ab3def",
///   "engine": "stockfish",
///   "fen": "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
///   "depth": 20,
///   "lines": [{ "pv": ["e7e5"], "evalCp": 30, "evalMate": null, "depth": 20 }]
/// }
///
/// Example analyze-batch-progress (server → client):
/// { "type": "analyze-batch-progress", "id": "req_lq5k2_ab3def", "completed": 3, "total": 10 }
///
/// Example analyze-batch-done (server → client):
/// {
///   "type": "analyze-batch-done",
///   "id": "req_lq5k2_ab3def",
///   "results": [
///     { "fen": "<fen1>", "engine": "stockfish", "lines": [...], "depth": 20 }
///   ]
/// }
///
/// Example error (server → client):
/// { "type": "error", "id": "req_lq5k2_ab3def", "code": "engine-crash", "message": "Stockfish exited unexpectedly" }
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HelperMessage {
    HelloAck {
        version: String,
        engines: Vec<HelperEngineInfo>,
    },
    AnalyzeProgress {
        id: String,
        engine: EngineId,
        fen: String,
        depth: u32,
        lines: Vec<EngineLine>,
    },
    AnalyzeDone {
        id: String,
        engine: EngineId,
        fen: String,
        depth: u32,
        lines: Vec<EngineLine>,
    },
    AnalyzeBatchProgress {
        id: String,
        completed: u32,
        total: u32,
    },
    AnalyzeBatchDone {
        id: String,
        results: Vec<BatchResult>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        code: String,
        message: String,
    },
}

/// One entry in analyze-batch-done results array.
/// TS: { fen: string; engine: EngineId; lines: EngineLine[]; depth: number }
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    pub fen: String,
    pub engine: EngineId,
    pub lines: Vec<EngineLine>,
    pub depth: u32,
}
