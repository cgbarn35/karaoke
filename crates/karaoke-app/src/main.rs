#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod fonts;
mod playback;

use std::path::PathBuf;

/// `--db <path>`, defaulting to `<dir of current exe>/karaoke.db`.
fn db_path() -> PathBuf {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--db"
            && let Some(path) = args.next()
        {
            return PathBuf::from(path);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("karaoke.db")))
        .unwrap_or_else(|| PathBuf::from("karaoke.db"))
}

fn main() -> eframe::Result {
    let db_path = db_path();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Karaoke")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Karaoke",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, db_path)))),
    )
}
