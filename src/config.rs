use std::path::PathBuf;

pub struct Config {
    pub stockfish_path: String,
    pub lc0_path: Option<String>,
    pub lc0_weights: Option<String>,
    pub bind_addr: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            stockfish_path: std::env::var("STOCKFISH_PATH")
                .unwrap_or_else(|_| default_path("stockfish")),
            lc0_path: std::env::var("LC0_PATH").ok().or_else(|| auto_detect("lc0")),
            lc0_weights: std::env::var("LC0_WEIGHTS").ok(),
            bind_addr: std::env::var("BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:9876".into()),
        }
    }
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
