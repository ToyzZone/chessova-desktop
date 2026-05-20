use std::path::PathBuf;

pub struct Config {
    pub stockfish_path: String,
    pub lc0_path: Option<String>,
    pub lc0_weights: Option<String>,
    pub bind_addr: String,
    /// Threads to give native Stockfish. Defaults to ncpu - 1 (leave one
    /// core for the OS / browser) capped at 16. Override via SF_THREADS.
    pub sf_threads: u32,
    /// Stockfish hash table size in MB. Bigger = deeper search reuses
    /// more positions. 512MB is a sweet spot for desktops in 2026.
    /// Override via SF_HASH_MB.
    pub sf_hash_mb: u32,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            stockfish_path: std::env::var("STOCKFISH_PATH")
                .unwrap_or_else(|_| default_path("stockfish")),
            lc0_path: std::env::var("LC0_PATH")
                .ok()
                .or_else(|| bundled_path("lc0"))
                .or_else(|| auto_detect("lc0")),
            lc0_weights: std::env::var("LC0_WEIGHTS")
                .ok()
                .or_else(|| bundled_weights_path()),
            bind_addr: std::env::var("BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:9876".into()),
            sf_threads: std::env::var("SF_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(default_threads),
            sf_hash_mb: std::env::var("SF_HASH_MB")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(512),
        }
    }
}

/// ncpu - 1, capped to [1, 16]. We leave one core for the OS and browser
/// so the user's machine stays responsive while analyzing.
fn default_threads() -> u32 {
    let n = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    n.saturating_sub(1).clamp(1, 16)
}

/// Bundled Lc0 weights file. On macOS the .app bundle stores data files
/// in Contents/Resources/ (so codesign --deep can sign the binaries
/// without choking on non-executable blobs). On Windows the zip layout
/// is flat, so weights sit next to the .exe. We probe both.
fn bundled_weights_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        // Flat layout (Windows zip, dev-from-source).
        dir.join("lc0-weights.pb.gz"),
        dir.join("lc0-weights.pb"),
        // macOS .app: weights live in ../Resources/.
        dir.join("../Resources/lc0-weights.pb.gz"),
        dir.join("../Resources/lc0-weights.pb"),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return candidate.canonicalize().ok()?.into_os_string().into_string().ok();
        }
    }
    None
}

fn default_path(bin: &str) -> String {
    bundled_path(bin)
        .or_else(|| auto_detect(bin))
        .unwrap_or_else(|| bin.into())
}

/// Look for `bin` next to the running binary. When the helper ships as a
/// `.app` bundle, stockfish lives at `Chessova Desktop.app/Contents/MacOS/stockfish`
/// — same directory as the helper executable itself. Bundled lookup
/// takes precedence so a user who has Homebrew Stockfish installed still
/// gets the version we tested against, not whatever's on PATH.
fn bundled_path(bin: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate: PathBuf = dir.join(bin);
    if candidate.is_file() {
        candidate.into_os_string().into_string().ok()
    } else {
        None
    }
}

fn auto_detect(bin: &str) -> Option<String> {
    for p in &["/opt/homebrew/bin", "/usr/local/bin"] {
        let full = format!("{}/{}", p, bin);
        if std::path::Path::new(&full).exists() {
            return Some(full);
        }
    }
    None
}
