//! Image thumbnails for the icon view.
//!
//! Decoding happens on worker threads and the result is handed back as raw
//! RGBA, because a `TextureHandle` can only be created on the UI thread. The
//! cache is bounded: every entry is a GPU texture, so it is the one place in
//! the app where unbounded growth would actually hurt.
//!
//! Types the `image` crate cannot decode - video, PDF, RAW, the whole of what
//! Explorer shows a preview for - are asked of the Windows shell instead, which
//! is where Explorer gets the same picture. A type with no thumbnail handler
//! answers `None` and the caller falls back to the per-type shell icon; outside
//! Windows there is no shell and the answer is always `None`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use crate::workers::Msg;

/// How many thumbnails to keep before the cache is emptied.
const CACHE_LIMIT: usize = 384;
/// Files above this size are not worth decoding for a 48px square.
const MAX_BYTES: u64 = 24 * 1024 * 1024;

/// A bounded, lazily filled thumbnail cache.
pub struct Thumbs {
    /// Decoded textures, keyed by path. `None` means "not an image, or failed".
    /// The size each was decoded at is kept, so a larger one can be asked for when
    /// the tiles grow.
    cache: HashMap<PathBuf, Option<(u32, egui::TextureHandle)>>,
    /// Paths a worker is already decoding, so we never queue the same one twice.
    inflight: HashMap<PathBuf, u32>,
    tx: Sender<Msg>,
}

impl Thumbs {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self {
            cache: HashMap::new(),
            inflight: HashMap::new(),
            tx,
        }
    }

    /// The texture for `path` sized to `px`, starting a decode when needed.
    ///
    /// Returns `None` while the work is in flight, which is what lets the
    /// caller fall back to a shell icon or a plain glyph for one frame.
    pub fn get(&mut self, path: &Path, px: u32) -> Option<egui::TextureHandle> {
        let mut stale = None;
        if let Some(hit) = self.cache.get(path) {
            match hit {
                Some((have, tex)) if *have >= px => return Some(tex.clone()),
                // Decoded smaller than is wanted now: shown as it is while the
                // bigger one is made, so the tile never goes blank.
                Some((_, tex)) => stale = Some(tex.clone()),
                None => return None,
            }
        }
        if self.inflight.get(path).is_some_and(|asked| *asked >= px) {
            return stale;
        }
        let image = is_image(path);
        if image {
            let Ok(md) = std::fs::metadata(crate::fs_model::long_path(path)) else {
                return stale;
            };
            if md.len() == 0 || md.len() > MAX_BYTES {
                self.cache.insert(path.to_path_buf(), None);
                return None;
            }
        }

        self.inflight.insert(path.to_path_buf(), px);
        let p = path.to_path_buf();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("rhumb-thumb".into())
            .spawn(move || {
                // The `image` crate reads the bytes; anything else is left to
                // the shell, which is the only thing that knows how to draw a
                // video frame or a PDF page.
                let decoded = if image {
                    decode(&p, px)
                } else {
                    shell_thumbnail(&p, px)
                };
                let msg = match decoded {
                    Some((rgba, w, h)) => Msg::Thumb {
                        path: p,
                        px,
                        rgba,
                        w,
                        h,
                    },
                    None => Msg::Thumb {
                        path: p,
                        px,
                        rgba: Vec::new(),
                        w: 0,
                        h: 0,
                    },
                };
                let _ = tx.send(msg);
            });
        if spawned.is_err() {
            self.inflight.remove(path);
        }
        stale
    }

    /// Whether `path` is known not to decode, so a caller can stop waiting on it.
    pub fn is_failed(&self, path: &Path) -> bool {
        matches!(self.cache.get(path), Some(None))
    }

    /// Stores a finished decode, evicting everything when the cache is full.
    pub fn insert(
        &mut self,
        path: PathBuf,
        px: u32,
        rgba: Vec<u8>,
        w: u32,
        h: u32,
        ctx: &egui::Context,
    ) {
        self.inflight.remove(&path);
        if rgba.is_empty() || w == 0 || h == 0 {
            // Remember the failure so the folder is not rescanned every frame, unless
            // there is a smaller picture already, which is better than none.
            if !matches!(self.cache.get(&path), Some(Some(_))) {
                self.cache.insert(path, None);
            }
            return;
        }
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        let tex = ctx.load_texture(
            format!("thumb:{}", px),
            image,
            egui::TextureOptions {
                magnification: egui::TextureFilter::Linear,
                minification: egui::TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::ClampToEdge,
                mipmap_mode: None,
            },
        );
        self.cache.insert(path, Some((px, tex)));
    }

    /// Drops everything, e.g. when the folder changes.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.inflight.clear();
    }
}

/// The size to decode a picture at for a place `points` wide on a screen that has
/// `ppp` pixels to the point: the next of a few steps up, so that growing a tile a
/// little does not decode everything again.
pub fn bucket(points: f32, ppp: f32) -> u32 {
    let want = (points * ppp).ceil().max(1.0) as u32;
    [64, 128, 256, 384, 512, 768, 1024, 1536, 2048]
        .into_iter()
        .find(|b| *b >= want)
        .unwrap_or(2048)
}

/// True for the extensions we can actually decode.
pub fn is_image(path: &Path) -> bool {
    matches!(
        crate::fs_model::ext_of(path).as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    )
}

/// Decodes and downscales to fit a `px` square, returning raw RGBA.
fn decode(path: &Path, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    let reader = image::ImageReader::open(crate::fs_model::long_path(path))
        .ok()?
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    shrink(img.into_raw(), w, h, px)
}

/// Shrinks raw RGBA to fit a `px` square, or hands it back untouched when it
/// already fits. Shared by the `image` decode and the shell thumbnail.
fn shrink(rgba: Vec<u8>, w: u32, h: u32, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    if w == 0 || h == 0 {
        return None;
    }
    // Only ever shrink: enlarging a 16px icon wastes memory and looks worse.
    let longest = w.max(h) as f32;
    if longest <= px as f32 {
        return Some((rgba, w, h));
    }
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let scale = px as f32 / longest;
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    Some((small.into_raw(), tw, th))
}

/// The Windows shell's own thumbnail for a file: the picture Explorer shows for
/// a video, a PDF or a RAW photo. `None` when the shell has no handler for the
/// type, which is what sends the caller on to the per-type shell icon.
///
/// Runs on the decode worker: `GetImage` can take a moment the first time, and
/// the apartment-threaded thumbnail handlers want COM set up on the thread that
/// calls them, so this thread initialises it for the length of the call.
#[cfg(windows)]
fn shell_thumbnail(path: &Path, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Graphics::Gdi::DeleteObject;
    use windows_sys::Win32::UI::Shell::SHCreateItemFromParsingName;

    // COM has to be up on this worker thread before the shell can be asked.
    let _com = com::init()?;

    // The shell's parser takes the ordinary path, not the `\\?\` form the file
    // APIs use; a path that is too long for it simply falls back to the icon.
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut factory: *mut shell_thumb::IShellItemImageFactory = std::ptr::null_mut();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path, the riid names the
    // interface asked for, and `factory` is a live out-pointer the call fills
    // with a reference that is released below.
    let hr = unsafe {
        SHCreateItemFromParsingName(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &shell_thumb::IID_ISHELLITEMIMAGEFACTORY,
            std::ptr::addr_of_mut!(factory).cast(),
        )
    };
    if hr < 0 || factory.is_null() {
        return None;
    }

    // SAFETY: `factory` is a live interface pointer from the call above.
    let bitmap = unsafe { shell_thumb::get_image(factory, px) };
    // The reference is ours to give up whatever `GetImage` answered.
    // SAFETY: `factory` is live and is released exactly once.
    unsafe { shell_thumb::release(factory) };

    let bitmap = bitmap?;
    // A thumbnail has no AND mask, unlike an icon.
    // SAFETY: `bitmap` is a live HBITMAP handed over by `GetImage`; it is read
    // and then deleted exactly once.
    let rgba = crate::shell_icons::bitmap_rgba(bitmap, std::ptr::null_mut());
    // SAFETY: `bitmap` is still live and is released exactly once.
    unsafe { DeleteObject(bitmap) };

    let (rgba, w, h) = rgba?;
    shrink(rgba, w, h, px)
}

/// No shell to ask outside Windows: the caller keeps its own glyph.
#[cfg(not(windows))]
fn shell_thumbnail(_path: &Path, _px: u32) -> Option<(Vec<u8>, u32, u32)> {
    None
}

/// COM for the shell-thumbnail worker.
#[cfg(windows)]
mod com {
    use windows_sys::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
    };

    /// COM initialisation that lasts as long as the value.
    ///
    /// The shell's thumbnail handlers are apartment-threaded, so the worker
    /// that asks for one enters an STA itself and tears it down when the
    /// picture is done, the way Explorer's own thumbnail thread does.
    pub(super) struct Guard {
        /// Whether `CoUninitialize` is ours to call. It is not when the thread
        /// was already in a different apartment, which the shell reports as
        /// `RPC_E_CHANGED_MODE`; COM is still usable there.
        uninit: bool,
    }

    /// Enters an STA on the calling thread, or `None` if COM cannot be used.
    pub(super) fn init() -> Option<Guard> {
        // SAFETY: a null reserved pointer and a valid COINIT value are the
        // documented arguments, and the call touches no memory of ours.
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if hr >= 0 {
            Some(Guard { uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            Some(Guard { uninit: false })
        } else {
            None
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            if self.uninit {
                // SAFETY: paired with the successful `CoInitializeEx` above.
                unsafe { CoUninitialize() };
            }
        }
    }
}

/// The one COM interface this file needs, spelled out by hand.
///
/// `windows-sys` carries the Win32 functions but not the COM interfaces, so the
/// vtable is written here. Only `Release` and `GetImage` are ever called, in
/// that order, but the three `IUnknown` slots must keep their places.
#[cfg(windows)]
mod shell_thumb {
    use core::ffi::c_void;

    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::HBITMAP;
    use windows_sys::Win32::UI::Shell::{SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY};
    use windows_sys::core::{GUID, HRESULT};

    /// `IShellItemImageFactory`, opaque: only its vtable is ever read.
    #[repr(C)]
    pub(super) struct IShellItemImageFactory {
        vtbl: *const Vtbl,
    }

    type QueryInterfaceFn =
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    type AddRefFn = unsafe extern "system" fn(*mut c_void) -> u32;
    type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
    type GetImageFn = unsafe extern "system" fn(*mut c_void, SIZE, SIIGBF, *mut HBITMAP) -> HRESULT;

    /// The vtable. `query_interface` and `add_ref` exist only to hold the
    /// `IUnknown` slots before `GetImage`; they are never called.
    #[allow(dead_code)]
    #[repr(C)]
    struct Vtbl {
        query_interface: QueryInterfaceFn,
        add_ref: AddRefFn,
        release: ReleaseFn,
        get_image: GetImageFn,
    }

    /// `{bcc18b79-ba16-442f-80c4-8a59c30c463b}`, from `shobjidl_core.h`.
    pub(super) const IID_ISHELLITEMIMAGEFACTORY: GUID =
        GUID::from_u128(0xbcc18b79_ba16_442f_80c4_8a59c30c463b);

    /// Asks for a thumbnail no larger than `px` square.
    ///
    /// `SIIGBF_THUMBNAILONLY` is what keeps a type with no thumbnail handler
    /// from answering with its plain icon: that fallback is the caller's to
    /// choose, not the shell's.
    ///
    /// # Safety
    /// `factory` must be a live `IShellItemImageFactory` the caller holds a
    /// reference to.
    pub(super) unsafe fn get_image(
        factory: *mut IShellItemImageFactory,
        px: u32,
    ) -> Option<HBITMAP> {
        // SAFETY: the caller guarantees `factory` is live, so its vtable
        // pointer is readable and valid for the object's lifetime.
        let vtbl = unsafe { (*factory).vtbl };
        if vtbl.is_null() {
            return None;
        }
        let size = SIZE {
            cx: px as i32,
            cy: px as i32,
        };
        let flags = SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK;
        let mut bitmap: HBITMAP = std::ptr::null_mut();
        // SAFETY: `factory` is live, `vtbl` is its valid vtable, and `bitmap`
        // is a live out-pointer of the right type.
        let hr = unsafe { ((*vtbl).get_image)(factory.cast(), size, flags, &mut bitmap) };
        if hr < 0 || bitmap.is_null() {
            None
        } else {
            Some(bitmap)
        }
    }

    /// Gives up one reference to the factory.
    ///
    /// # Safety
    /// `factory` must be a live interface pointer the caller owns a reference
    /// to, and it must not be used afterwards.
    pub(super) unsafe fn release(factory: *mut IShellItemImageFactory) {
        // SAFETY: the caller guarantees `factory` is live, so its vtable
        // pointer is readable and valid for the object's lifetime.
        let vtbl = unsafe { (*factory).vtbl };
        if !vtbl.is_null() {
            // SAFETY: the vtable is valid and this gives up the reference that
            // `SHCreateItemFromParsingName` handed over, exactly once.
            unsafe { ((*vtbl).release)(factory.cast()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_decodable_extensions_count_as_images() {
        assert!(is_image(Path::new("a.PNG")));
        assert!(is_image(Path::new("b.jpeg")));
        assert!(is_image(Path::new("c.webp")));
        assert!(!is_image(Path::new("d.txt")));
        assert!(!is_image(Path::new("noext")));
    }

    #[test]
    fn decoding_shrinks_but_never_enlarges() {
        let src = image::RgbaImage::from_fn(64, 32, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 8) as u8, 128, 255])
        });
        let path = std::env::temp_dir().join("rhumb-thumb-test.png");
        src.save(&path).expect("write png");

        // Downscaling fits the long edge to the target, keeping the aspect.
        let (_, w, h) = decode(&path, 16).expect("decodes");
        assert_eq!((w, h), (16, 8));
        // A target larger than the source leaves it alone.
        let (_, w, h) = decode(&path, 512).expect("decodes");
        assert_eq!((w, h), (64, 32));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_decodes_to_nothing() {
        assert!(decode(Path::new("no-such-file-anywhere.png"), 64).is_none());
    }

    #[test]
    fn a_failure_is_remembered_so_it_is_not_asked_for_again() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut t = Thumbs::new(tx);
        let ctx = egui::Context::default();
        // A path that is not on the disk: no decode and no shell thumbnail can
        // succeed, so the worker's empty answer stands for every failure.
        let path = std::env::temp_dir().join("rhumb-thumb-does-not-exist.txt");
        let _ = std::fs::remove_file(&path);
        assert!(t.get(&path, 64).is_none(), "the worker has not run yet");
        let Msg::Thumb {
            path: p,
            px,
            rgba,
            w,
            h,
        } = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("a thumbnail answer was expected")
        else {
            panic!("a thumbnail was expected");
        };
        t.insert(p, px, rgba, w, h, &ctx);
        assert!(t.is_failed(&path), "the failure is remembered");
        // Asked again: the remembered failure is answered at once, no worker.
        assert!(t.get(&path, 64).is_none());
        assert!(
            rx.try_recv().is_err(),
            "the failed path was asked for a second time"
        );
    }

    #[test]
    fn the_cache_is_emptied_when_it_fills() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut t = Thumbs::new(tx);
        let ctx = egui::Context::default();
        // Fill past the limit; the cache clears itself rather than growing.
        for i in 0..CACHE_LIMIT + 5 {
            t.insert(
                PathBuf::from(format!("x{i}.png")),
                64,
                vec![255; 4],
                1,
                1,
                &ctx,
            );
        }
        assert!(
            t.cache.len() < CACHE_LIMIT,
            "the cache did not bound itself: {}",
            t.cache.len()
        );
    }
}

/// The shell thumbnail can only be exercised where there is a shell. The tests
/// take turns: the shell is happiest when asked from one thread, which is how
/// the app asks, and the test runner would otherwise race several.
#[cfg(all(test, windows))]
mod shell_tests {
    use super::*;

    static SHELL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SHELL.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_type_with_no_thumbnail_handler_falls_back_without_panicking() {
        let _guard = lock();
        let path = std::env::temp_dir().join("rhumb-shell-thumb-none.txt");
        std::fs::write(&path, b"just some text").expect("write txt");
        // A text file has no thumbnail provider, so `SIIGBF_THUMBNAILONLY`
        // answers nothing. `None` is the fallback signal, not a panic.
        assert!(shell_thumbnail(&path, 64).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_shell_path_returns_a_picture_end_to_end() {
        let _guard = lock();
        // A bitmap is one the `image` crate also decodes, so it does not prove
        // the app's routing, but it does exercise the whole shell path: create
        // the item, call `GetImage`, and turn the `HBITMAP` into RGBA. A type
        // the `image` crate cannot decode and the shell can (PDF, video, TIFF)
        // cannot be written with the crates this project carries.
        let path = std::env::temp_dir().join("rhumb-shell-thumb.bmp");
        image::RgbaImage::from_pixel(48, 24, image::Rgba([10, 200, 30, 255]))
            .save(&path)
            .expect("write bmp");
        let (rgba, w, h) = shell_thumbnail(&path, 64).expect("the shell thumbnails a bmp");
        assert!(w > 0 && h > 0, "empty picture: {w}x{h}");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn a_picture_is_decoded_at_the_next_size_up_from_the_place_it_fills() {
        assert_eq!(bucket(100.0, 1.0), 128);
        assert_eq!(bucket(100.0, 1.5), 256, "150 pixels wants the 256 step");
        assert_eq!(bucket(300.0, 2.0), 768);
        assert_eq!(bucket(4000.0, 2.0), 2048, "and no more than the largest");
    }

    #[test]
    fn a_larger_size_is_asked_for_when_the_tiles_grow() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut t = Thumbs::new(tx);
        let dir = std::env::temp_dir().join(format!("rhumb-thumb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        image::RgbaImage::from_pixel(600, 400, image::Rgba([200, 30, 30, 255]))
            .save(&path)
            .unwrap();
        let ctx = egui::Context::default();
        assert!(t.get(&path, 128).is_none(), "the decode is on a worker");
        let Msg::Thumb {
            path: p,
            px,
            rgba,
            w,
            h,
        } = rx.recv().unwrap()
        else {
            panic!("a thumbnail was expected");
        };
        assert_eq!(px, 128);
        assert!(w.max(h) <= 128 && w.max(h) > 100, "decoded at {w}x{h}");
        t.insert(p, px, rgba, w, h, &ctx);
        assert!(t.get(&path, 128).is_some(), "the small one is there");
        // Asked for larger: the small one is shown meanwhile, and a bigger decode starts.
        assert!(
            t.get(&path, 512).is_some(),
            "the smaller picture is kept on show"
        );
        let Msg::Thumb { px, w, h, .. } = rx.recv().unwrap() else {
            panic!("a larger thumbnail was expected");
        };
        assert_eq!(px, 512);
        assert!(w.max(h) > 400, "decoded at {w}x{h}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
