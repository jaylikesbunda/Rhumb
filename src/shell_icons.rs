//! Real Windows shell icons for the file list.
//!
//! Every file used to draw the same generic glyph and every folder the same
//! folder glyph. Windows already knows the picture it puts beside a `.pdf`, an
//! `.exe` or a folder, so it is asked for that picture once per type and the
//! result is kept as a texture. Unlike a thumbnail decode the shell call is
//! synchronous and quick, so it needs no worker thread: a small map from
//! extension to texture is enough, and it is bounded because every entry is a
//! GPU texture.
//!
//! Outside Windows there is no shell to ask, so every lookup answers `None` and
//! the caller keeps drawing its own glyph.

use std::collections::HashMap;

use crate::fs_model::Entry;

/// How many icons to keep before the cache is emptied. A folder holds far fewer
/// distinct types than this, and a fresh lookup is cheap.
const CACHE_LIMIT: usize = 256;

/// A bounded cache of shell icons: one per file extension, plus the single
/// folder icon.
#[derive(Default)]
struct ShellIcons {
    /// File icons by lower-case extension. `None` means "the shell gave
    /// nothing", remembered so it is not asked again every frame.
    files: HashMap<String, Option<egui::TextureHandle>>,
    /// The folder icon. `None` until it is asked for; `Some(None)` when the
    /// shell gave nothing.
    folder: Option<Option<egui::TextureHandle>>,
}

impl ShellIcons {
    /// The shell icon for a file with this extension, loading it on first ask.
    fn file(&mut self, ext: &str, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        // `Entry::ext` already lower-cases, so the common case finds its key
        // without building a `String` on every frame.
        if let Some(hit) = self.files.get(ext) {
            return hit.clone();
        }
        let ext = ext.to_ascii_lowercase();
        if let Some(hit) = self.files.get(&ext) {
            return hit.clone();
        }
        let tex = shell_texture(&format!("x.{ext}"), false, ctx);
        if self.files.len() >= CACHE_LIMIT {
            self.files.clear();
        }
        self.files.insert(ext, tex.clone());
        tex
    }

    /// The shell icon for a folder, loading it on first ask.
    fn folder(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if let Some(hit) = &self.folder {
            return hit.clone();
        }
        // With `SHGFI_USEFILEATTRIBUTES` the name need not exist; only the
        // directory attribute decides which icon comes back.
        let tex = shell_texture("x", true, ctx);
        self.folder = Some(tex.clone());
        tex
    }
}

thread_local! {
    /// One cache per UI thread. egui lives on the main thread, and a
    /// thread-local keeps this out of every call site's state.
    static ICONS: std::cell::RefCell<ShellIcons> = std::cell::RefCell::new(ShellIcons::default());
}

/// The shell icon for a file with this extension (lower-case, no dot).
pub fn file_icon(ext: &str, ctx: &egui::Context) -> Option<egui::TextureHandle> {
    ICONS.with_borrow_mut(|icons| icons.file(ext, ctx))
}

/// The shell icon for a folder.
pub fn folder_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    ICONS.with_borrow_mut(|icons| icons.folder(ctx))
}

/// The shell icon for a listed entry: the folder icon for a directory, and
/// otherwise the icon Windows shows for its extension.
pub fn entry_icon(entry: &Entry, ctx: &egui::Context) -> Option<egui::TextureHandle> {
    if entry.is_dir {
        folder_icon(ctx)
    } else {
        // The extension is read straight off the path: `Entry::ext` lower-cases
        // by building a `String`, which this path is asked for every row.
        let ext = entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        file_icon(ext, ctx)
    }
}

/// Loads one icon and turns it into a texture. `None` on any failure, which is
/// what lets the caller fall back to the drawn glyph.
fn shell_texture(name: &str, is_dir: bool, ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let (rgba, w, h) = shell_rgba(name, is_dir)?;
    if rgba.is_empty() || w == 0 || h == 0 {
        return None;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some(ctx.load_texture(
        format!("shell:{name}:{is_dir}"),
        image,
        egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: None,
        },
    ))
}

/// Asks the shell for the small icon of a representative name and returns it as
/// straight RGBA. Never panics: a shell failure is just `None`.
#[cfg(windows)]
fn shell_rgba(name: &str, is_dir: bool) -> Option<(Vec<u8>, u32, u32)> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::UI::Shell::{
        SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let attrs = if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    let mut info = SHFILEINFOW::default();
    // SAFETY: `wide` is a NUL-terminated UTF-16 name and `info` is a live
    // `SHFILEINFOW` whose exact size is passed, so the shell writes only that.
    let got = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
        )
    };
    if got == 0 || info.hIcon.is_null() {
        return None;
    }
    // SAFETY: `info.hIcon` is a live icon handle that `SHGetFileInfoW` hands
    // over to us; it is converted and then destroyed exactly once.
    let rgba = icon_rgba(info.hIcon);
    // SAFETY: the handle is still live and is destroyed exactly once.
    unsafe {
        DestroyIcon(info.hIcon);
    }
    rgba
}

/// No shell outside Windows: the caller draws its own glyph.
#[cfg(not(windows))]
fn shell_rgba(_name: &str, _is_dir: bool) -> Option<(Vec<u8>, u32, u32)> {
    None
}

/// Splits a live `HICON` into its colour bitmap and AND mask and reads the
/// colour bitmap out as RGBA, using the mask only for icons with no alpha.
#[cfg(windows)]
fn icon_rgba(
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
) -> Option<(Vec<u8>, u32, u32)> {
    use windows_sys::Win32::Graphics::Gdi::DeleteObject;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    // SAFETY: `icon` is a live HICON and `info` is a live `ICONINFO`; the two
    // bitmaps it receives become ours to release below.
    let mut info = ICONINFO::default();
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        return None;
    }
    let color = info.hbmColor;
    let mask = info.hbmMask;
    let rgba = if color.is_null() {
        None
    } else {
        bitmap_rgba(color, mask)
    };
    // `GetIconInfo` creates both bitmaps; they are ours to release even when
    // the conversion failed.
    // SAFETY: each non-null handle is a live GDI bitmap owned by us and is
    // released exactly once.
    unsafe {
        if !color.is_null() {
            DeleteObject(color);
        }
        if !mask.is_null() {
            DeleteObject(mask);
        }
    }
    rgba
}

/// Reads a live colour bitmap, with its matching AND mask, out as straight RGBA.
///
/// Shared with the shell-thumbnail path in `thumbs.rs`, which hands over the
/// `HBITMAP` that `IShellItemImageFactory::GetImage` returns.
#[cfg(windows)]
pub(crate) fn bitmap_rgba(
    color: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    mask: windows_sys::Win32::Graphics::Gdi::HBITMAP,
) -> Option<(Vec<u8>, u32, u32)> {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        GetDIBits, GetObjectW,
    };

    // SAFETY: `color` is a live bitmap and `BITMAP` is the right size for
    // `GetObjectW` to fill with its geometry.
    let mut bm = BITMAP::default();
    let got = unsafe {
        GetObjectW(
            color,
            std::mem::size_of::<BITMAP>() as i32,
            std::ptr::addr_of_mut!(bm).cast(),
        )
    };
    if got == 0 {
        return None;
    }
    let w = bm.bmWidth.max(0) as u32;
    let h = bm.bmHeight.max(0) as u32;
    if w == 0 || h == 0 {
        return None;
    }

    // SAFETY: a null HDC is allowed and asks for a memory DC compatible with
    // the screen; the returned DC is released below.
    let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    if dc.is_null() {
        return None;
    }

    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w as i32;
    // A negative height asks for top-down rows, the order egui wants.
    bmi.bmiHeader.biHeight = -(h as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // SAFETY: the buffer is exactly `w * h * 4` bytes and `bmi` describes that
    // same 32-bit top-down format, so `GetDIBits` fills only the buffer.
    let lines = unsafe {
        GetDIBits(
            dc,
            color,
            0,
            h,
            rgba.as_mut_ptr().cast(),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    if lines == 0 {
        // SAFETY: `dc` was created above and is released exactly once.
        unsafe { DeleteDC(dc) };
        return None;
    }

    // A 32-bit icon carries its own alpha. An older one does not, and its shape
    // lives in the AND mask instead, where a zero bit means opaque.
    if rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 0) && !mask.is_null() {
        let mut mask_bits = vec![0u8; rgba.len()];
        // SAFETY: the same 32-bit format as above, into a buffer of the same
        // size; GDI expands the 1-bit mask to black and white.
        let mask_lines = unsafe {
            GetDIBits(
                dc,
                mask,
                0,
                h,
                mask_bits.as_mut_ptr().cast(),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        if mask_lines != 0 {
            for (px, m) in rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(mask_bits.as_chunks::<4>().0)
            {
                if m[0] == 0 {
                    px[3] = 0xff;
                }
            }
        }
    }

    // SAFETY: `dc` was created above and is released exactly once.
    unsafe { DeleteDC(dc) };

    // `GetDIBits` hands back BGRA; egui wants RGBA.
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some((rgba, w, h))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The shell's very first call from several threads at once can come back
    /// with a null icon handle. The app only ever asks from the one UI thread,
    /// so the tests take turns to model that rather than race.
    static SHELL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SHELL.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_known_extension_gets_a_real_shell_icon() {
        let _guard = lock();
        let ctx = egui::Context::default();
        let first = file_icon("rs", &ctx).expect("the shell has an icon for .rs");
        let size = first.size_vec2();
        assert!(size.x > 0.0 && size.y > 0.0, "empty texture: {size:?}");
        // Asked again, the cached handle comes back rather than a second one.
        let second = file_icon("rs", &ctx).expect("the cached icon");
        assert_eq!(first.id(), second.id(), "the second ask reloaded the icon");
    }

    #[test]
    fn an_unknown_or_empty_extension_falls_back_without_panicking() {
        let _guard = lock();
        let ctx = egui::Context::default();
        // Either a generic icon or nothing: both are fine, neither panics.
        let _ = file_icon("", &ctx);
        let _ = file_icon("not-a-real-extension-xyz", &ctx);
        let _ = folder_icon(&ctx);
    }
}
