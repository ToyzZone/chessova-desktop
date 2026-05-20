//! eframe-based progress window shown during one-click engine installs.
//! Spawned from the tray click handler on a dedicated OS thread (eframe
//! takes over a thread's event loop).
//!
//! Subscribes to a `tokio::sync::watch::Receiver<InstallProgress>` and
//! repaints each frame, surfacing download bytes / rate, extract phase,
//! and final success or failure.

use crate::installer::InstallProgress;
use eframe::egui;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Open a progress window. Blocks the calling thread until the user
/// closes the window or auto-close fires post-success. Safe to call
/// from a worker thread — eframe takes the thread's event loop.
pub fn run(engine_label: String, rx: watch::Receiver<InstallProgress>) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 200.0])
            .with_resizable(false)
            .with_title(format!("Installing {}", engine_label)),
        ..Default::default()
    };

    let app_state = ProgressApp {
        engine_label,
        rx,
        done_at: None,
    };

    if let Err(e) = eframe::run_native(
        "Chessova Desktop Installer",
        options,
        Box::new(|_cc| Box::new(app_state)),
    ) {
        tracing::warn!(err = %e, "progress window failed");
    }
}

struct ProgressApp {
    engine_label: String,
    rx: watch::Receiver<InstallProgress>,
    /// When `Done`, the time of transition. Used to auto-close after 2s
    /// so success doesn't require a manual dismiss.
    done_at: Option<Instant>,
}

impl eframe::App for ProgressApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let progress = self.rx.borrow().clone();

        // Track Done timestamp once.
        if matches!(progress, InstallProgress::Done { .. }) && self.done_at.is_none() {
            self.done_at = Some(Instant::now());
        }

        // Auto-close after 2s post-success.
        if let Some(t) = self.done_at {
            if t.elapsed() > Duration::from_secs(2) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading(format!("Installing {}", self.engine_label));
            ui.add_space(12.0);

            match &progress {
                InstallProgress::Idle => {
                    ui.label("Preparing…");
                }
                InstallProgress::Downloading {
                    bytes_done,
                    bytes_total,
                    rate_bps,
                } => {
                    let fraction = bytes_total
                        .filter(|t| *t > 0)
                        .map(|t| (*bytes_done as f32 / t as f32).clamp(0.0, 1.0));

                    if let Some(f) = fraction {
                        ui.add(
                            egui::ProgressBar::new(f)
                                .desired_width(ui.available_width())
                                .show_percentage(),
                        );
                    } else {
                        // Indeterminate animation when content-length is unknown.
                        ui.add(
                            egui::ProgressBar::new(animated_indeterminate(ctx))
                                .desired_width(ui.available_width()),
                        );
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }

                    ui.add_space(6.0);
                    ui.label(format!(
                        "Downloading…   {} / {} · {}/s",
                        format_bytes(*bytes_done),
                        bytes_total
                            .map(format_bytes)
                            .unwrap_or_else(|| "?".into()),
                        format_bytes(*rate_bps),
                    ));
                }
                InstallProgress::Extracting => {
                    ui.add(
                        egui::ProgressBar::new(animated_indeterminate(ctx))
                            .desired_width(ui.available_width()),
                    );
                    ctx.request_repaint_after(Duration::from_millis(50));
                    ui.add_space(6.0);
                    ui.label("Extracting archive…");
                }
                InstallProgress::Verifying => {
                    ui.add(
                        egui::ProgressBar::new(1.0)
                            .desired_width(ui.available_width()),
                    );
                    ui.add_space(6.0);
                    ui.label("Verifying installed binary…");
                }
                InstallProgress::Done { path } => {
                    ui.add(
                        egui::ProgressBar::new(1.0)
                            .desired_width(ui.available_width())
                            .text("Done"),
                    );
                    ui.add_space(6.0);
                    ui.label(format!("✓ Installed to {}", short_path(path)));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("This window will close automatically.")
                            .small()
                            .weak(),
                    );
                }
                InstallProgress::Failed(msg) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 70, 70),
                        format!("✗ Install failed: {}", msg),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Close").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
            }

            // Cancel button only while in-flight. (v0.4.0 closes the
            // window but the install task continues to completion;
            // proper cancellation lands in v0.4.1 — see plan.)
            if !progress.is_terminal() {
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
        });

        // Keep repainting at modest rate while not terminal so progress
        // updates from the watch channel get rendered.
        if !progress.is_terminal() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

/// Smooth back-and-forth value in [0.3, 0.7] for the indeterminate bar.
fn animated_indeterminate(ctx: &egui::Context) -> f32 {
    let t = ctx.input(|i| i.time);
    0.5 + 0.2 * (t as f32 * 2.0).sin()
}

/// Human-friendly byte size: "2.6 MB" / "412 KB" etc.
fn format_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.0} KB", n as f64 / KB as f64)
    } else {
        format!("{} B", n)
    }
}

fn short_path(path: &str) -> String {
    if path.len() <= 60 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }
    let tail: Vec<&str> = parts.iter().rev().take(3).rev().copied().collect();
    format!("…/{}", tail.join("/"))
}
