//! Status bar tray icon. macOS: NSStatusItem in the menu bar (top-right).
//! Windows: system tray (bottom-right). Cross-platform via the
//! tray-icon + tao crates.
//!
//! Runs on the main thread (Cocoa / Win32 event loops require it). The
//! tokio runtime lives on a background thread and we hand its Handle to
//! the click handlers so they can spawn install tasks.

use crate::config::Config;
use crate::engine_status::{BinaryStatus, EngineStatus, Health};
use crate::installer::{self, InstallProgress};
use crate::lc0_weights::{self, WeightsChoiceId};
use crate::progress_window;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tray_icon::{
    menu::{
        Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
    },
    Icon, TrayIconBuilder, TrayIconEvent,
};

const HELP_URL: &str = "https://github.com/ToyzZone/chessova-desktop#troubleshooting";
const WEIGHTS_HELP_URL: &str =
    "https://github.com/ToyzZone/chessova-desktop#about-lc0-weights";

/// Shared install progress, one entry per engine being installed.
type InstallStateMap = HashMap<EngineKind, watch::Receiver<InstallProgress>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EngineKind {
    Stockfish,
    Lc0Binary,
    Lc0Weights,
}

/// Per-rebuild menu identifiers — needed because tray-icon's MenuId
/// values are generated at construction time, so we capture them after
/// each rebuild for the event handler to match against.
struct MenuIds {
    quit: MenuId,
    help: Option<MenuId>,
    install_sf: Option<MenuId>,
    install_lc0_binary: Option<MenuId>,
    weights_choices: HashMap<MenuId, WeightsChoiceId>,
    weights_help: Option<MenuId>,
}

pub fn run(status: EngineStatus, runtime: Handle) -> Result<()> {
    let install_state: Arc<Mutex<InstallStateMap>> = Arc::new(Mutex::new(HashMap::new()));
    // Holds the current EngineStatus — re-probed after each install so
    // the menu reflects newly-available engines.
    let status: Arc<Mutex<EngineStatus>> = Arc::new(Mutex::new(status));

    let (menu, mut ids) = build_menu(&status.lock().unwrap(), &install_state.lock().unwrap());
    let icon = build_icon(status.lock().unwrap().health())?;
    let tooltip = tooltip_for(status.lock().unwrap().health());

    let tray = TrayIconBuilder::new()
        .with_tooltip(tooltip)
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()?;

    let event_loop = EventLoopBuilder::new().build();
    let menu_channel = MenuEvent::receiver();
    let _tray_channel = TrayIconEvent::receiver();

    // Probe the install state map periodically so we can detect a Done
    // event from the watch channel and refresh the engine status.
    let proxy = event_loop.create_proxy();
    let install_state_for_ticker = install_state.clone();
    let status_for_ticker = status.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        let mut any_terminal = false;
        {
            let map = install_state_for_ticker.lock().unwrap();
            for rx in map.values() {
                if rx.borrow().is_terminal() {
                    any_terminal = true;
                    break;
                }
            }
        }
        if any_terminal {
            // Drop all terminal entries and re-probe engine status.
            let mut map = install_state_for_ticker.lock().unwrap();
            map.retain(|_, rx| !rx.borrow().is_terminal());
            drop(map);
            let cfg = Config::from_env();
            *status_for_ticker.lock().unwrap() = EngineStatus::probe(&cfg);
            let _ = proxy.send_event(());
        }
    });

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        // Tao user events fire when the ticker detects an install
        // terminal — rebuild the menu so freshly-installed engines stop
        // showing the "Install …" item.
        if matches!(event, tao::event::Event::UserEvent(_)) {
            let s = status.lock().unwrap();
            let map = install_state.lock().unwrap();
            let (new_menu, new_ids) = build_menu(&s, &map);
            tray.set_menu(Some(Box::new(new_menu)));
            if let Ok(new_icon) = build_icon(s.health()) {
                let _ = tray.set_icon(Some(new_icon));
            }
            let _ = tray.set_tooltip(Some(tooltip_for(s.health())));
            ids = new_ids;
        }

        while let Ok(menu_event) = menu_channel.try_recv() {
            if menu_event.id == ids.quit {
                tracing::info!("quit requested via tray");
                std::process::exit(0);
            }
            if let Some(hid) = &ids.help {
                if &menu_event.id == hid {
                    let _ = open_url(HELP_URL);
                    continue;
                }
            }
            if let Some(sid) = &ids.install_sf {
                if &menu_event.id == sid {
                    spawn_install(
                        EngineKind::Stockfish,
                        "Stockfish",
                        InstallKind::Stockfish,
                        &runtime,
                        &install_state,
                    );
                    continue;
                }
            }
            if let Some(lid) = &ids.install_lc0_binary {
                if &menu_event.id == lid {
                    spawn_install(
                        EngineKind::Lc0Binary,
                        "Lc0",
                        InstallKind::Lc0Binary,
                        &runtime,
                        &install_state,
                    );
                    continue;
                }
            }
            if let Some(choice_id) = ids.weights_choices.get(&menu_event.id) {
                if let Some(choice) = lc0_weights::lookup(*choice_id) {
                    spawn_install(
                        EngineKind::Lc0Weights,
                        choice.label,
                        InstallKind::Lc0Weights(*choice_id),
                        &runtime,
                        &install_state,
                    );
                }
                continue;
            }
            if let Some(wh) = &ids.weights_help {
                if &menu_event.id == wh {
                    let _ = open_url(WEIGHTS_HELP_URL);
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Install spawn
// ---------------------------------------------------------------------------

enum InstallKind {
    Stockfish,
    Lc0Binary,
    Lc0Weights(WeightsChoiceId),
}

fn spawn_install(
    kind: EngineKind,
    label: &str,
    install: InstallKind,
    runtime: &Handle,
    install_state: &Arc<Mutex<InstallStateMap>>,
) {
    // Skip if this engine is already installing.
    {
        let map = install_state.lock().unwrap();
        if let Some(rx) = map.get(&kind) {
            if !rx.borrow().is_terminal() {
                tracing::info!(?kind, "install already in progress");
                return;
            }
        }
    }

    let (tx, rx) = watch::channel(InstallProgress::Idle);
    install_state.lock().unwrap().insert(kind, rx.clone());

    // Spawn the install task on the tokio runtime.
    let tx_for_task = tx;
    runtime.spawn(async move {
        let result = match install {
            InstallKind::Stockfish => installer::install_stockfish(tx_for_task).await,
            InstallKind::Lc0Binary => installer::install_lc0_binary(tx_for_task).await,
            InstallKind::Lc0Weights(id) => installer::install_lc0_weights(id, tx_for_task).await,
        };
        if let Err(e) = &result {
            tracing::warn!(err = %e, "install failed");
        }
    });

    // Spawn the progress window on a dedicated OS thread — eframe owns
    // the thread's event loop.
    let window_label = label.to_string();
    std::thread::spawn(move || {
        progress_window::run(window_label, rx);
    });
}

// ---------------------------------------------------------------------------
// Menu construction
// ---------------------------------------------------------------------------

fn build_menu(status: &EngineStatus, install_state: &InstallStateMap) -> (Menu, MenuIds) {
    let menu = Menu::new();

    let header = MenuItem::new(header_text(status), false, None);
    let _ = menu.append(&header);
    let _ = menu.append(&PredefinedMenuItem::separator());

    // Stockfish row + maybe install action.
    let sf_label = sf_row_label(status, install_state.get(&EngineKind::Stockfish));
    let _ = menu.append(&MenuItem::new(sf_label, false, None));
    let install_sf = if needs_install(&status.stockfish, install_state.get(&EngineKind::Stockfish))
    {
        let item = MenuItem::new("Install Stockfish (~6 MB)", true, None);
        let id = item.id().clone();
        let _ = menu.append(&item);
        Some(id)
    } else {
        None
    };

    // Lc0 binary row + maybe install action.
    let lc0_label = lc0_row_label(status, install_state.get(&EngineKind::Lc0Binary));
    let _ = menu.append(&MenuItem::new(lc0_label, false, None));
    let install_lc0_binary = if needs_install(&status.lc0, install_state.get(&EngineKind::Lc0Binary))
    {
        let item = MenuItem::new("Install Lc0 engine (~20 MB)", true, None);
        let id = item.id().clone();
        let _ = menu.append(&item);
        Some(id)
    } else {
        None
    };

    // Lc0 weights row + ALWAYS-available "Install / Switch Lc0 weights" submenu.
    let weights_label = weights_row_label(status, install_state.get(&EngineKind::Lc0Weights));
    let _ = menu.append(&MenuItem::new(weights_label, false, None));
    let (weights_submenu_ids, weights_help_id) = build_weights_submenu(&menu, status);

    let _ = menu.append(&PredefinedMenuItem::separator());

    let help = if needs_help(status) {
        let item = MenuItem::new("Get help installing engines…", true, None);
        let id = item.id().clone();
        let _ = menu.append(&item);
        Some(id)
    } else {
        None
    };

    let _ = menu.append(&MenuItem::new(
        "Endpoint  ws://127.0.0.1:9876/uci",
        false,
        None,
    ));

    let _ = menu.append(&PredefinedMenuItem::separator());
    let quit_item = MenuItem::new("Quit Chessova Desktop", true, None);
    let quit_id = quit_item.id().clone();
    let _ = menu.append(&quit_item);

    (
        menu,
        MenuIds {
            quit: quit_id,
            help,
            install_sf,
            install_lc0_binary,
            weights_choices: weights_submenu_ids,
            weights_help: weights_help_id,
        },
    )
}

fn build_weights_submenu(
    parent: &Menu,
    status: &EngineStatus,
) -> (HashMap<MenuId, WeightsChoiceId>, Option<MenuId>) {
    let mut choices: HashMap<MenuId, WeightsChoiceId> = HashMap::new();
    let label = if status.lc0_weights.is_found() {
        "Switch Lc0 weights"
    } else {
        "Install Lc0 weights"
    };
    let sub = Submenu::new(label, true);
    for choice in lc0_weights::CATALOG {
        let item = MenuItem::new(choice.label, true, None);
        let id = item.id().clone();
        let _ = sub.append(&item);
        choices.insert(id, choice.id);
    }
    let _ = sub.append(&PredefinedMenuItem::separator());
    let help_item = MenuItem::new("About Lc0 weights…", true, None);
    let help_id = help_item.id().clone();
    let _ = sub.append(&help_item);
    let _ = parent.append(&sub);
    (choices, Some(help_id))
}

// ---------------------------------------------------------------------------
// Label helpers
// ---------------------------------------------------------------------------

fn sf_row_label(status: &EngineStatus, install: Option<&watch::Receiver<InstallProgress>>) -> String {
    if let Some(progress_label) = active_install_label(install) {
        return format!("Stockfish  {}", progress_label);
    }
    format!("Stockfish  {}", status.stockfish.label())
}

fn lc0_row_label(status: &EngineStatus, install: Option<&watch::Receiver<InstallProgress>>) -> String {
    if let Some(progress_label) = active_install_label(install) {
        return format!("Lc0  {}", progress_label);
    }
    format!("Lc0  {}", status.lc0.label())
}

fn weights_row_label(
    status: &EngineStatus,
    install: Option<&watch::Receiver<InstallProgress>>,
) -> String {
    if let Some(progress_label) = active_install_label(install) {
        return format!("Lc0 weights  {}", progress_label);
    }
    let base = status.lc0_weights.label();
    match installer::active_weights_label() {
        Some(name) if status.lc0_weights.is_found() => format!("Lc0 weights  ✓ {}", name),
        _ => format!("Lc0 weights  {}", base),
    }
}

fn active_install_label(install: Option<&watch::Receiver<InstallProgress>>) -> Option<String> {
    let rx = install?;
    let progress = rx.borrow().clone();
    match progress {
        InstallProgress::Idle => None,
        InstallProgress::Downloading { bytes_done, bytes_total, .. } => {
            let pct = bytes_total
                .filter(|t| *t > 0)
                .map(|t| ((bytes_done as f64 / t as f64) * 100.0).round() as u32);
            match pct {
                Some(p) => Some(format!("⌛ downloading… {}%", p)),
                None => Some("⌛ downloading…".to_string()),
            }
        }
        InstallProgress::Extracting => Some("⌛ extracting…".to_string()),
        InstallProgress::Verifying => Some("⌛ verifying…".to_string()),
        InstallProgress::Done { .. } => None,
        InstallProgress::Failed(_) => Some("✗ install failed".to_string()),
    }
}

fn needs_install(
    status: &BinaryStatus,
    install: Option<&watch::Receiver<InstallProgress>>,
) -> bool {
    // Don't show install action while a download is in flight for this engine.
    if let Some(rx) = install {
        if !rx.borrow().is_terminal() {
            return false;
        }
    }
    matches!(status, BinaryStatus::Missing { .. } | BinaryStatus::NotConfigured)
}

fn header_text(status: &EngineStatus) -> String {
    match status.health() {
        Health::Ok => "● Ready".into(),
        Health::Degraded => "◐ Stockfish only (Lc0 unavailable)".into(),
        Health::Broken => "✗ Stockfish missing".into(),
    }
}

fn needs_help(status: &EngineStatus) -> bool {
    matches!(status.health(), Health::Broken | Health::Degraded)
}

fn tooltip_for(health: Health) -> &'static str {
    match health {
        Health::Ok => "Chessova Desktop · ready",
        Health::Degraded => "Chessova Desktop · Lc0 unavailable",
        Health::Broken => "Chessova Desktop · Stockfish missing",
    }
}

// ---------------------------------------------------------------------------
// Icon + URL helpers (unchanged from v0.3.0)
// ---------------------------------------------------------------------------

/// Build a 22x22 RGBA icon. Filled circle whose color reflects engine health.
fn build_icon(health: Health) -> Result<Icon> {
    let (r, g, b) = match health {
        Health::Ok => (60u8, 200u8, 110u8),
        Health::Degraded => (220u8, 170u8, 40u8),
        Health::Broken => (220u8, 70u8, 70u8),
    };

    let size = 22u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let radius = (size as f32 / 2.0) - 1.5;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = if d <= radius - 0.5 {
                255
            } else if d >= radius + 0.5 {
                0
            } else {
                let t = (radius + 0.5 - d).clamp(0.0, 1.0);
                (t * 255.0) as u8
            };
            rgba.extend_from_slice(&[r, g, b, alpha]);
        }
    }

    Icon::from_rgba(rgba, size, size)
        .map_err(|e| anyhow::anyhow!("icon construction failed: {}", e))
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()?;
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
    }
    Ok(())
}
