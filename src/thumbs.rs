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
    /// Returns `None` while the decode is in flight, which is what lets the
    /// caller fall back to a plain glyph for one frame.
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
        if !is_image(path) {
            self.cache.insert(path.to_path_buf(), None);
            return None;
        }
        let Ok(md) = std::fs::metadata(path) else {
            return stale;
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
        stale
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
        let dir = std::env::temp_dir().join(format!("xplor-thumb-{}", std::process::id()));
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
