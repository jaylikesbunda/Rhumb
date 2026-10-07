//! Rhumb: a minimal, fast, dark file explorer with a built-in text and
//! Markdown editor.

// A release build is a windowed program, so it does not open a console behind
// the window. Debug builds keep the console for log output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![warn(clippy::all)]

mod app;
mod archive;
mod buffer;
mod clip;
mod codeedit;
mod editing;
mod editor;
mod fs_model;
mod index;
mod instance;
mod markdown;
mod ops;
mod recycle;
mod remote;
mod search;
mod shell_icons;
mod theme;
mod this_pc;
mod thumbs;
mod tree;
mod typeahead;
mod widgets;
mod workers;

#[cfg(test)]
mod prop_tests;

use eframe::egui::{IconData, ViewportBuilder};

/// The application icon, embedded so the binary is self-contained.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

fn main() -> eframe::Result {
    init_logging();

    // One window per user. A second launch hands its path to the first and
    // exits, rather than opening a second window with its own history and
    // search box.
    let start = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if !instance::claim(start.as_deref()) {
        log::info!("another instance is running; handed the path and exiting");
        return Ok(());
    }

    let icon = load_icon();
    let viewport = ViewportBuilder::default()
        .with_title("Rhumb")
        .with_app_id("dev.rhumb.explorer")
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
        wgpu_options: wgpu_options(),
        ..Default::default()
    };

    eframe::run_native(
        "rhumb",
        native,
        Box::new(|cc| {
            instance::watch(cc.egui_ctx.clone());
            #[cfg(windows)]
            set_window_icons(cc);
            Ok(Box::new(app::Rhumb::new(cc)))
        }),
    )
}

/// Restricts wgpu to Direct3D 12 on Windows. By default it brings up every
/// backend (Vulkan, GL, DX12) and enumerates their adapters, which is most of
/// the time a window takes to appear. Every supported Windows has DX12.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut options = eframe::egui_wgpu::WgpuConfiguration::default()
        .with_surface_config(eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY);
    #[cfg(windows)]
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
    options
}

/// Gives the window the icons embedded in the executable, at the exact sizes
/// the title bar and taskbar ask for at the current scale. Handing the window
/// one bitmap leaves Windows to rescale it, and the result is softer and
/// differs from what Explorer shows for the same file.
#[cfg(windows)]
fn set_window_icons(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON,
        SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };

    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win) = handle.as_raw() else {
        return;
    };
    let hwnd = win.hwnd.get() as *mut std::ffi::c_void;
    // SAFETY: plain Win32 calls on the window this process just created, with
    // the icon resource id 1 that assets/rhumb.rc defines.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        for (kind, cx, cy) in [
            (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
            (ICON_BIG, SM_CXICON, SM_CYICON),
        ] {
            // `MAKEINTRESOURCE(1)`: a resource id, not a pointer to anything, so the
            // address is the id and there is no provenance to carry.
            let icon = LoadImageW(
                module,
                std::ptr::without_provenance(1),
                IMAGE_ICON,
                GetSystemMetrics(cx),
                GetSystemMetrics(cy),
                LR_DEFAULTCOLOR,
            );
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, kind as usize, icon as isize);
            }
        }
    }
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
        .join("rhumb")
        .join("rhumb.log");
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
    log::info!("rhumb {} starting", env!("CARGO_PKG_VERSION"));

    // `panic = "abort"` in release makes a panic a silent exit with no console to
    // print to, so the reason is written to the log before the process goes. The
    // log lock is not held across this, and a poisoned lock just drops the write.
    std::panic::set_hook(Box::new(|info| {
        let name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let location = info
            .location()
            .map_or_else(String::new, |l| format!(" at {l}"));
        log::error!("panic in thread {name}{location}: {info}");
        if std::env::var_os("RUST_BACKTRACE").is_some() {
            log::error!("backtrace:\n{}", std::backtrace::Backtrace::force_capture());
        }
        eprintln!("rhumb panicked{location}: {info}");
    }));
}
