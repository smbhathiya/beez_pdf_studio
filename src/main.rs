// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;

mod pdf;
mod ui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("UtilBee PDF Studio")
            .with_inner_size([1200.0, 820.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "UtilBee PDF Studio",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
}
