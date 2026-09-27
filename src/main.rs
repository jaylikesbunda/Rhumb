//! Xplor: a minimal, fast, dark file explorer with a built-in text and
//! Markdown editor.

#![warn(clippy::all)]

mod app;
mod editing;
mod editor;
mod fs_model;
mod markdown;
mod ops;
mod search;
mod theme;
mod thumbs;
mod tree;
mod typeahead;
mod widgets;
mod workers;

use eframe::egui::{IconData, ViewportBuilder};

/// The application icon, embedded so the binary is self-contained.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn main() -> eframe::Result {
    init_logging();

    let icon = load_icon();
    let viewport = ViewportBuilder::default()
        .with_title("Xplor")
        .with_app_id("dev.xplor.explorer")
        .with_inner_size([1160.0, 720.0])
        .with_min_inner_size([720.0, 440.0])
        // egui-winit ignores `titlebar_shown`, so the only route to a custom
        // title bar is an undecorated window. Winit keeps the resize border
        // and window snapping for resizable undecorated windows, and the app
        // draws matching edge handles as a fallback.
        .with_decorations(false)
        .with_resizable(true)
        .with_drag_and_drop(true);

    let viewport = match icon {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };

    let native = eframe::NativeOptions {
        viewport,
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "xplor",
        native,
        Box::new(|cc| Ok(Box::new(app::Xplor::new(cc)))),
    )
}

/// Decodes the embedded icon for the window and taskbar.
fn load_icon() -> Option<IconData> {
    let decoded = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).ok()?;
    let rgba = decoded.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(IconData {
        rgba: rgba.into_raw(),
        width: w,
        height: h,
    })
}

/// A log sink that serialises writes from the logger thread.
struct FileSink(std::sync::Mutex<std::fs::File>);

impl std::io::Write for FileSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut f = self
            .0
            .lock()
            .map_err(|_| std::io::Error::other("log file lock poisoned"))?;
        f.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut f = self
            .0
            .lock()
            .map_err(|_| std::io::Error::other("log file lock poisoned"))?;
        f.flush()
    }
}

/// Logs to a file under the user data directory, so problems can be diagnosed
/// after the fact without a console window.
fn init_logging() {
    use std::io::Write;
    let path = dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("xplor")
        .join("xplor.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let mut builder = env_logger::Builder::new();
    builder.filter_level(level);
    builder.target(env_logger::Target::Pipe(Box::new(FileSink(
        std::sync::Mutex::new(file),
    ))));
    builder.format(move |buf, record| {
        writeln!(
            buf,
            "{} [{}] {}",
            buf.timestamp(),
            record.level(),
            record.args()
        )
    });
    let _ = builder.try_init();
    log::info!("xplor {} starting", env!("CARGO_PKG_VERSION"));
}
