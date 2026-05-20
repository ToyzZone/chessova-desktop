//! OS-aware writable directory for runtime-installed engine binaries.
//! Used by the one-click installer to drop Stockfish / Lc0 into a path
//! that survives helper updates and doesn't break the .app's ad-hoc
//! signature (writing into Contents/MacOS would invalidate it).

use std::path::PathBuf;

/// Returns the directory where the runtime installer places downloaded
/// engines. On macOS: `~/Library/Application Support/Chessova/engines`.
/// On Windows: `%APPDATA%\Chessova\engines`. Linux: `~/.local/share/chessova/engines`.
///
/// The dir is created on first call (idempotent). Returns `None` if
/// `dirs::data_dir()` fails — extremely rare; only happens on
/// misconfigured systems without `$HOME`.
pub fn user_engines_dir() -> Option<PathBuf> {
    let base = dirs::data_dir()?;
    let dir = base.join("Chessova").join("engines");
    if !dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(err = %e, path = %dir.display(), "failed to create engines dir");
            return None;
        }
    }
    Some(dir)
}
