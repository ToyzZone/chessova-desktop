use crate::config::Config;
use std::path::Path;

/// Runtime engine availability snapshot. Probed at startup so the tray
/// UI can show users which engines are loaded and surface install
/// instructions when something's missing.
#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub stockfish: BinaryStatus,
    pub lc0: BinaryStatus,
    pub lc0_weights: BinaryStatus,
}

#[derive(Debug, Clone)]
pub enum BinaryStatus {
    /// Resolved + the file exists.
    Found { path: String },
    /// Path was resolved but the file is missing (broken install).
    Missing { path: String },
    /// Optional engine that wasn't configured at all (e.g. Lc0 without
    /// install). Not an error — surfaced as "not installed" in the UI.
    NotConfigured,
}

impl BinaryStatus {
    pub fn label(&self) -> String {
        match self {
            BinaryStatus::Found { path } => format!("✓  {}", short_path(path)),
            BinaryStatus::Missing { path } => format!("✗  missing ({})", short_path(path)),
            BinaryStatus::NotConfigured => "—  not installed".to_string(),
        }
    }

    pub fn is_found(&self) -> bool {
        matches!(self, BinaryStatus::Found { .. })
    }
}

impl EngineStatus {
    pub fn probe(config: &Config) -> Self {
        Self {
            stockfish: probe_required(&config.stockfish_path),
            lc0: match &config.lc0_path {
                Some(p) => probe_required(p),
                None => BinaryStatus::NotConfigured,
            },
            lc0_weights: match &config.lc0_weights {
                Some(p) => probe_required(p),
                None => BinaryStatus::NotConfigured,
            },
        }
    }

    /// Overall health: green if SF found, yellow if SF found but Lc0
    /// missing (degraded), red if SF missing (broken).
    pub fn health(&self) -> Health {
        if !self.stockfish.is_found() {
            return Health::Broken;
        }
        match (&self.lc0, &self.lc0_weights) {
            (BinaryStatus::NotConfigured, _) | (_, BinaryStatus::NotConfigured) => Health::Ok,
            (BinaryStatus::Missing { .. }, _) | (_, BinaryStatus::Missing { .. }) => {
                Health::Degraded
            }
            _ => Health::Ok,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Degraded,
    Broken,
}

fn probe_required(path: &str) -> BinaryStatus {
    if Path::new(path).is_file() {
        BinaryStatus::Found { path: path.into() }
    } else {
        BinaryStatus::Missing { path: path.into() }
    }
}

/// Shorten a long absolute path for display. Keeps the last 2-3
/// components ("…/MacOS/stockfish") so users can identify it without
/// the menu growing absurdly wide.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }
    let tail: Vec<&str> = parts.iter().rev().take(3).rev().copied().collect();
    format!("…/{}", tail.join("/"))
}
