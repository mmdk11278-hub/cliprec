#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotations;
mod app;
mod audio_loopback;
mod capture;
mod editor;
mod icon;
mod mf;
mod recorder;
mod storage;
mod theme;

use std::path::PathBuf;

use app::ClipRecApp;

fn parse_screenshot_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--screenshot" {
            if let Some(p) = args.next() {
                return Some(PathBuf::from(p));
            }
        } else if let Some(rest) = arg.strip_prefix("--screenshot=") {
            if !rest.is_empty() {
                return Some(PathBuf::from(rest));
            }
        }
    }
    None
}

fn main() -> eframe::Result<()> {
    let screenshot_path = parse_screenshot_arg();
    let is_screenshot = screenshot_path.is_some();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("ClipRec — Screen Capture & Recording Studio")
        .with_icon(std::sync::Arc::new(icon::app_icon_data()))
        .with_inner_size([1120.0, 680.0])
        .with_min_inner_size([840.0, 540.0]);

    if is_screenshot {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "ClipRec",
        options,
        Box::new(move |cc| Ok(Box::new(ClipRecApp::new(cc, screenshot_path)))),
    )
}
