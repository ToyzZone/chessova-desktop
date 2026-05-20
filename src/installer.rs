//! One-click engine installer. Downloads Stockfish or Lc0 binaries
//! + weights from upstream releases and places them in the user's
//! Application Support directory (see `install_paths`).
//!
//! Progress is published via a `tokio::sync::watch` channel so both
//! the tray menu (polls every 500 ms) and the progress window (repaints
//! at ~60 Hz) see the latest state without blocking on a queue.

use crate::install_paths;
use crate::lc0_weights::{self, WeightsChoice, WeightsChoiceId};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum InstallProgress {
    Idle,
    Downloading {
        bytes_done: u64,
        bytes_total: Option<u64>,
        rate_bps: u64,
    },
    Extracting,
    Verifying,
    Done {
        path: String,
    },
    Failed(String),
}

impl InstallProgress {
    pub fn is_terminal(&self) -> bool {
        matches!(self, InstallProgress::Done { .. } | InstallProgress::Failed(_))
    }
}

// ---------------------------------------------------------------------------
// Engine download URLs (pinned to known-good releases)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
const STOCKFISH_URL: &str =
    "https://github.com/official-stockfish/Stockfish/releases/download/sf_18/stockfish-macos-m1-apple-silicon.tar";

#[cfg(target_os = "windows")]
const STOCKFISH_URL: &str =
    "https://github.com/official-stockfish/Stockfish/releases/download/sf_18/stockfish-windows-x86-64-sse41-popcnt.zip";

#[cfg(target_os = "macos")]
const LC0_URL: &str =
    "https://github.com/LeelaChessZero/lc0/releases/download/v0.32.1/lc0-v0.32.1-mac-arm-metal.tar.gz";

#[cfg(target_os = "windows")]
const LC0_URL: &str =
    "https://github.com/LeelaChessZero/lc0/releases/download/v0.32.1/lc0-v0.32.1-windows-cpu-dnnl.zip";

// ---------------------------------------------------------------------------
// Top-level install functions
// ---------------------------------------------------------------------------

/// Install Stockfish into the user engines dir. Returns the final path
/// of the installed `stockfish` (or `stockfish.exe`) binary.
pub async fn install_stockfish(tx: watch::Sender<InstallProgress>) -> Result<PathBuf> {
    let result = install_stockfish_inner(&tx).await;
    publish_result(&tx, &result);
    result
}

/// Install the Lc0 binary (no weights) into the user engines dir.
pub async fn install_lc0_binary(tx: watch::Sender<InstallProgress>) -> Result<PathBuf> {
    let result = install_lc0_binary_inner(&tx).await;
    publish_result(&tx, &result);
    result
}

/// Install a specific Lc0 weights file. Writes to `lc0-weights.pb.gz`
/// (the canonical name config.rs probes for) and updates the
/// `lc0-weights.meta` sidecar so the tray can show which weights are
/// active.
pub async fn install_lc0_weights(
    id: WeightsChoiceId,
    tx: watch::Sender<InstallProgress>,
) -> Result<PathBuf> {
    let choice = lc0_weights::lookup(id)
        .ok_or_else(|| anyhow!("unknown weights id: {:?}", id))?;
    let result = install_lc0_weights_inner(choice, &tx).await;
    publish_result(&tx, &result);
    result
}

fn publish_result(tx: &watch::Sender<InstallProgress>, result: &Result<PathBuf>) {
    let msg = match result {
        Ok(p) => InstallProgress::Done {
            path: p.display().to_string(),
        },
        Err(e) => InstallProgress::Failed(e.to_string()),
    };
    let _ = tx.send(msg);
}

// ---------------------------------------------------------------------------
// Stockfish
// ---------------------------------------------------------------------------

async fn install_stockfish_inner(tx: &watch::Sender<InstallProgress>) -> Result<PathBuf> {
    let dir = install_paths::user_engines_dir()
        .ok_or_else(|| anyhow!("could not resolve user engines directory"))?;
    let bytes = download_to_memory(STOCKFISH_URL, tx).await?;

    let _ = tx.send(InstallProgress::Extracting);

    // Final filename matches what config.rs probes for.
    #[cfg(target_os = "macos")]
    let final_path = dir.join("stockfish");
    #[cfg(target_os = "windows")]
    let final_path = dir.join("stockfish.exe");

    // SF macOS ships as .tar (uncompressed); SF Windows ships as .zip.
    #[cfg(target_os = "macos")]
    {
        extract_tar_member(&bytes, &dir, &final_path, |name| {
            // Path inside the tar looks like "stockfish/stockfish-macos-m1-apple-silicon"
            let lower = name.to_ascii_lowercase();
            !lower.ends_with('/') && lower.contains("stockfish") && !lower.contains("readme")
        })?;
    }
    #[cfg(target_os = "windows")]
    {
        extract_zip_member(&bytes, &dir, &final_path, |name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".exe") && lower.contains("stockfish")
        })?;
    }

    let _ = tx.send(InstallProgress::Verifying);
    verify_executable(&final_path)?;
    Ok(final_path)
}

// ---------------------------------------------------------------------------
// Lc0 binary
// ---------------------------------------------------------------------------

async fn install_lc0_binary_inner(tx: &watch::Sender<InstallProgress>) -> Result<PathBuf> {
    let dir = install_paths::user_engines_dir()
        .ok_or_else(|| anyhow!("could not resolve user engines directory"))?;
    let bytes = download_to_memory(LC0_URL, tx).await?;

    let _ = tx.send(InstallProgress::Extracting);

    #[cfg(target_os = "macos")]
    let final_path = dir.join("lc0");
    #[cfg(target_os = "windows")]
    let final_path = dir.join("lc0.exe");

    #[cfg(target_os = "macos")]
    {
        // macOS archive is .tar.gz. Extract the `lc0` binary and skip
        // the bundled weights — the user picks weights separately.
        extract_targz_member(&bytes, &dir, &final_path, |name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with("/lc0") || lower == "lc0"
        })?;
    }
    #[cfg(target_os = "windows")]
    {
        // Windows zip contains lc0.exe + dnnl.dll + mimalloc-*.dll.
        // Extract all DLLs alongside the exe so lc0 can find them.
        extract_zip_with_dlls(&bytes, &dir)?;
    }

    let _ = tx.send(InstallProgress::Verifying);
    verify_executable(&final_path)?;
    Ok(final_path)
}

// ---------------------------------------------------------------------------
// Lc0 weights
// ---------------------------------------------------------------------------

async fn install_lc0_weights_inner(
    choice: &WeightsChoice,
    tx: &watch::Sender<InstallProgress>,
) -> Result<PathBuf> {
    let dir = install_paths::user_engines_dir()
        .ok_or_else(|| anyhow!("could not resolve user engines directory"))?;
    let bytes = download_to_memory(choice.url, tx).await?;

    let _ = tx.send(InstallProgress::Extracting);

    let final_path = dir.join("lc0-weights.pb.gz");
    let tmp_path = dir.join(".tmp-lc0-weights.pb.gz");
    std::fs::write(&tmp_path, &bytes).with_context(|| format!("writing {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("renaming into {}", final_path.display()))?;

    // Sidecar metadata so the tray can show which weights are active.
    let meta_path = dir.join("lc0-weights.meta");
    let _ = std::fs::write(&meta_path, lc0_weights::short_label(choice.label));

    let _ = tx.send(InstallProgress::Verifying);
    if !final_path.is_file() {
        return Err(anyhow!("weights file missing after install"));
    }
    Ok(final_path)
}

/// Read the active Lc0 weights label from the sidecar. None when the
/// helper is using its bundled weights (no sidecar present).
pub fn active_weights_label() -> Option<String> {
    let dir = install_paths::user_engines_dir()?;
    let meta = dir.join("lc0-weights.meta");
    std::fs::read_to_string(meta).ok().map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// HTTP download with progress reporting
// ---------------------------------------------------------------------------

async fn download_to_memory(url: &str, tx: &watch::Sender<InstallProgress>) -> Result<Bytes> {
    let _ = tx.send(InstallProgress::Downloading {
        bytes_done: 0,
        bytes_total: None,
        rate_bps: 0,
    });

    let response = reqwest::get(url)
        .await
        .with_context(|| format!("GET {}", url))?
        .error_for_status()
        .with_context(|| format!("non-2xx from {}", url))?;
    let total = response.content_length();

    let mut buffer: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = response.bytes_stream();
    let start = Instant::now();
    let mut last_emit = Instant::now();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("network error while downloading")?;
        buffer.extend_from_slice(&chunk);
        // Throttle emits to ~30 Hz so we don't spam the watch channel.
        if last_emit.elapsed().as_millis() > 33 {
            let elapsed = start.elapsed().as_secs_f64().max(0.001);
            let rate_bps = (buffer.len() as f64 / elapsed) as u64;
            let _ = tx.send(InstallProgress::Downloading {
                bytes_done: buffer.len() as u64,
                bytes_total: total,
                rate_bps,
            });
            last_emit = Instant::now();
        }
    }

    Ok(Bytes::from(buffer))
}

// ---------------------------------------------------------------------------
// Archive extraction (atomic via temp file + rename)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn extract_tar_member(
    bytes: &Bytes,
    dir: &Path,
    final_path: &Path,
    predicate: impl Fn(&str) -> bool,
) -> Result<()> {
    let mut archive = tar::Archive::new(Cursor::new(bytes.as_ref()));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if !predicate(&path) {
            continue;
        }
        return extract_to(&mut entry, dir, final_path);
    }
    Err(anyhow!("no matching entry in tar"))
}

#[cfg(target_os = "macos")]
fn extract_targz_member(
    bytes: &Bytes,
    dir: &Path,
    final_path: &Path,
    predicate: impl Fn(&str) -> bool,
) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes.as_ref()));
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if !predicate(&path) {
            continue;
        }
        return extract_to(&mut entry, dir, final_path);
    }
    Err(anyhow!("no matching entry in tar.gz"))
}

#[cfg(target_os = "macos")]
fn extract_to<R: Read>(entry: &mut R, dir: &Path, final_path: &Path) -> Result<()> {
    let tmp = dir.join(format!(".tmp-{}", final_path.file_name().unwrap().to_string_lossy()));
    let mut out = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    std::io::copy(entry, &mut out).context("write extracted entry")?;
    out.flush()?;
    drop(out);
    set_executable(&tmp)?;
    std::fs::rename(&tmp, final_path)
        .with_context(|| format!("rename into {}", final_path.display()))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn extract_zip_member(
    bytes: &Bytes,
    dir: &Path,
    final_path: &Path,
    predicate: impl Fn(&str) -> bool,
) -> Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.as_ref()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if !predicate(&name) {
            continue;
        }
        let tmp = dir.join(format!(
            ".tmp-{}",
            final_path.file_name().unwrap().to_string_lossy()
        ));
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut entry, &mut out)?;
        out.flush()?;
        drop(out);
        std::fs::rename(&tmp, final_path)?;
        return Ok(());
    }
    Err(anyhow!("no matching entry in zip"))
}

#[cfg(target_os = "windows")]
fn extract_zip_with_dlls(bytes: &Bytes, dir: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.as_ref()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        let lower = name.to_ascii_lowercase();
        let final_name = if lower.ends_with("lc0.exe") {
            "lc0.exe"
        } else if lower.ends_with(".dll") {
            // Keep DLL filenames as-is (dnnl.dll, mimalloc-*.dll).
            std::path::Path::new(&name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            continue;
        };
        if final_name.is_empty() {
            continue;
        }
        let final_path = dir.join(final_name);
        let tmp = dir.join(format!(".tmp-{}", final_name));
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut entry, &mut out)?;
        out.flush()?;
        drop(out);
        std::fs::rename(&tmp, &final_path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verify + permissions
// ---------------------------------------------------------------------------

fn verify_executable(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("expected file at {} after install", path.display()));
    }
    #[cfg(unix)]
    set_executable(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}
