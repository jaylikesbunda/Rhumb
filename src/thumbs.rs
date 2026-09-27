//! Image thumbnails for the icon view.
//!
//! Decoding happens on worker threads and the result is handed back as raw
//! RGBA, because a `TextureHandle` can only be created on the UI thread. The
//! cache is bounded: every entry is a GPU texture, so it is the one place in
//! the app where unbounded growth would actually hurt.

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
    cache: HashMap<PathBuf, Option<egui::TextureHandle>>,
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
    /// Returns `None` while the decode is in flight, which is what lets the
    /// caller fall back to a plain glyph for one frame.
    pub fn get(&mut self, path: &Path, px: u32) -> Option<egui::TextureHandle> {
        if let Some(hit) = self.cache.get(path) {
            return hit.clone();
        }
        if self.inflight.contains_key(path) {
            return None;
        }
        if !is_image(path) {
            self.cache.insert(path.to_path_buf(), None);
            return None;
        }
        let Ok(md) = std::fs::metadata(path) else {
            return None;
        };
        if md.len() == 0 || md.len() > MAX_BYTES {
            self.cache.insert(path.to_path_buf(), None);
            return None;
        }

        self.inflight.insert(path.to_path_buf(), px);
        let p = path.to_path_buf();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("xplor-thumb".into())
            .spawn(move || {
                let msg = match decode(&p, px) {
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
        None
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
            // Remember the failure so the folder is not rescanned every frame.
            self.cache.insert(path, None);
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
        self.cache.insert(path, Some(tex));
    }

    /// Drops everything, e.g. when the folder changes.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.inflight.clear();
    }
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
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    // Only ever shrink: enlarging a 16px icon wastes memory and looks worse.
    let longest = w.max(h) as f32;
    if longest <= px as f32 {
        return Some((img.into_raw(), w, h));
    }
    let scale = px as f32 / longest;
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    Some((small.into_raw(), tw, th))
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
        let path = std::env::temp_dir().join("xplor-thumb-test.png");
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
}
