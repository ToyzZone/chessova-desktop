//! Catalog of Lc0 weights options the user can pick from in the tray
//! menu. Each entry maps a human-friendly label (shown in the submenu)
//! to a stable download URL and a brief explanation.
//!
//! Used by `tray.rs` to build the submenu and by `installer.rs` to
//! resolve a `WeightsChoiceId` back to its URL.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeightsChoiceId {
    T1_256,
    T1_512,
    BT4,
    Maia1100,
    Maia1500,
    Maia1900,
}

#[derive(Debug, Clone)]
pub struct WeightsChoice {
    pub id: WeightsChoiceId,
    /// Display name shown in the tray submenu. Includes size + character
    /// hint so users can pick without reading docs.
    pub label: &'static str,
    pub url: &'static str,
    pub approx_mb: u32,
}

/// Static catalog. Order matters — the tray submenu renders them in
/// this order, top-to-bottom.
pub const CATALOG: &[WeightsChoice] = &[
    WeightsChoice {
        id: WeightsChoiceId::T1_256,
        label: "T1-256 · fast, ~30 MB · recommended",
        url: "https://storage.lczero.org/files/networks-contrib/t1-256x10-distilled-swa-2432500.pb.gz",
        approx_mb: 30,
    },
    WeightsChoice {
        id: WeightsChoiceId::T1_512,
        label: "T1-512 · stronger, ~100 MB · slower on CPU",
        url: "https://storage.lczero.org/files/networks-contrib/t1-smolgen-512x15x8h-distilled-swa-3395000.pb.gz",
        approx_mb: 100,
    },
    WeightsChoice {
        id: WeightsChoiceId::BT4,
        label: "BT4 · very strong, ~350 MB · GPU recommended",
        url: "https://storage.lczero.org/files/networks-contrib/BT4-1024x15x32h-swa-6147500.pb.gz",
        approx_mb: 350,
    },
    WeightsChoice {
        id: WeightsChoiceId::Maia1100,
        label: "Maia 1100 · plays like 1100-rated human · ~30 MB",
        url: "https://github.com/CSSLab/maia-chess/releases/download/v1.0/maia-1100.pb.gz",
        approx_mb: 30,
    },
    WeightsChoice {
        id: WeightsChoiceId::Maia1500,
        label: "Maia 1500 · plays like 1500-rated human · ~30 MB",
        url: "https://github.com/CSSLab/maia-chess/releases/download/v1.0/maia-1500.pb.gz",
        approx_mb: 30,
    },
    WeightsChoice {
        id: WeightsChoiceId::Maia1900,
        label: "Maia 1900 · plays like 1900-rated human · ~30 MB",
        url: "https://github.com/CSSLab/maia-chess/releases/download/v1.0/maia-1900.pb.gz",
        approx_mb: 30,
    },
];

pub fn lookup(id: WeightsChoiceId) -> Option<&'static WeightsChoice> {
    CATALOG.iter().find(|c| c.id == id)
}

/// Strip the suffix after the bullet so the tray's "active weights"
/// label stays short. e.g. "Maia 1500 · plays like…" → "Maia 1500".
pub fn short_label(label: &str) -> &str {
    label.split(" · ").next().unwrap_or(label)
}
