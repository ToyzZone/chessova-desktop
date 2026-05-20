//! Status bar tray icon. macOS: NSStatusItem in the menu bar (top-right).
//! Windows: system tray (bottom-right). Cross-platform via the
//! tray-icon + tao crates.
//!
//! Runs on the main thread (Cocoa / Win32 event loops require it). The
//! tokio runtime lives on a background thread and feeds the engine
//! status here at startup.

use crate::engine_status::{BinaryStatus, EngineStatus, Health};
use anyhow::Result;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder, TrayIconEvent,
};

const HELP_URL: &str = "https://github.com/ToyzZone/chessova-desktop#troubleshooting";

pub fn run(status: EngineStatus) -> Result<()> {
    // Build menu reflecting engine status. Items are static after
    // construction — the helper doesn't re-probe at runtime in this
    // first version. (A future version could poll periodically.)
    let menu = Menu::new();

    let header = MenuItem::new(header_text(&status), false, None);
    menu.append(&header)?;
    menu.append(&PredefinedMenuItem::separator())?;

    let sf_item = MenuItem::new(format!("Stockfish  {}", status.stockfish.label()), false, None);
    menu.append(&sf_item)?;

    let lc0_item = MenuItem::new(format!("Lc0  {}", status.lc0.label()), false, None);
    menu.append(&lc0_item)?;

    let weights_item =
        MenuItem::new(format!("Lc0 weights  {}", status.lc0_weights.label()), false, None);
    menu.append(&weights_item)?;

    menu.append(&PredefinedMenuItem::separator())?;

    // Conditional help item: shows when something is broken so the user
    // can click through to install guidance.
    let help_item = if needs_help(&status) {
        Some(MenuItem::new("Get help installing engines…", true, None))
    } else {
        None
    };
    if let Some(item) = &help_item {
        menu.append(item)?;
    }

    let endpoint_item = MenuItem::new(
        "Endpoint  ws://127.0.0.1:9876/uci",
        false,
        None,
    );
    menu.append(&endpoint_item)?;

    menu.append(&PredefinedMenuItem::separator())?;
    let quit_item = MenuItem::new("Quit Chessova Desktop", true, None);
    menu.append(&quit_item)?;

    let quit_id = quit_item.id().clone();
    let help_id = help_item.as_ref().map(|i| i.id().clone());

    let tooltip = match status.health() {
        Health::Ok => "Chessova Desktop · ready",
        Health::Degraded => "Chessova Desktop · Lc0 unavailable",
        Health::Broken => "Chessova Desktop · Stockfish missing",
    };

    // Build the tray icon. We use a text-symbol icon (♞ chess knight)
    // baked into a tiny PNG. tray-icon needs an icon — none means
    // nothing shows up. The dot color hints at status.
    let icon = build_icon(status.health())?;
    let _tray = TrayIconBuilder::new()
        .with_tooltip(tooltip)
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .build()?;

    let event_loop = EventLoopBuilder::new().build();
    let menu_channel = MenuEvent::receiver();
    let _tray_channel = TrayIconEvent::receiver();

    event_loop.run(move |_event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        while let Ok(menu_event) = menu_channel.try_recv() {
            if menu_event.id == quit_id {
                tracing::info!("quit requested via tray");
                std::process::exit(0);
            }
            if let Some(hid) = &help_id {
                if &menu_event.id == hid {
                    if let Err(e) = open_url(HELP_URL) {
                        tracing::warn!(err = %e, "failed to open help URL");
                    }
                }
            }
        }
    });
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
        || matches!(status.stockfish, BinaryStatus::Missing { .. })
}

/// Build a 16x16 RGBA icon. We draw a simple filled circle whose color
/// reflects engine health. No external assets — the icon is generated
/// in-process so the .app/.zip stays self-contained.
fn build_icon(health: Health) -> Result<tray_icon::Icon> {
    let (r, g, b) = match health {
        Health::Ok => (60u8, 200u8, 110u8),       // green
        Health::Degraded => (220u8, 170u8, 40u8), // amber
        Health::Broken => (220u8, 70u8, 70u8),    // red
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
            // Anti-alias: full opacity inside, fade out over ~1px at the edge.
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

    tray_icon::Icon::from_rgba(rgba, size, size)
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
