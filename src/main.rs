#![windows_subsystem = "windows"]

mod app;
mod config;
mod fabric;
mod friends;
mod horizon;
mod hz_mods;
mod java;
#[cfg(target_os = "macos")]
mod mac;
mod minecraft;
mod mods;
mod native;
mod net;
mod skin;
mod system;
mod task;
mod tunnel;
mod ui;
mod viewer;
mod window;
mod zoom;

use eframe::egui;

/// The "Z" mark from assets/logo.webp, prepared by build.rs.
pub const LOGO_MARK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/logo_mark.rgba"));
pub const LOGO_MARK_SIZE: u32 = 128;

fn icon() -> egui::IconData {
    egui::IconData { rgba: LOGO_MARK.to_vec(), width: LOGO_MARK_SIZE, height: LOGO_MARK_SIZE }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(window::TITLE)
            .with_inner_size([1180.0, 700.0])
            .with_maximized(true)
            .with_min_inner_size([980.0, 620.0])
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native("Zyntra Launcher", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
