//! Remote images for the Markdown preview.
//!
//! A Markdown file may point at an image on the web — a badge, or a picture
//! hosted on GitHub. Fetching and decoding happen on worker threads, and the
//! result is handed back as raw RGBA because a `TextureHandle` can only be made
//! on the UI thread. The cache is bounded: every entry is a GPU texture.
//!
//! SVG is supported too, because the badge services only serve SVG. It is
//! rasterised with `resvg`, using the fonts the app already ships so the badge
//! text renders.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use crate::theme;
use crate::workers::Msg;

/// How many remote pictures to keep before the cache is emptied.
const CACHE_LIMIT: usize = 128;
/// Downloads above this size are dropped rather than decoded.
const MAX_BYTES: usize = 12 * 1024 * 1024;

/// A bounded, lazily filled cache of remote images, keyed by URL.
#[derive(Default)]
pub struct RemoteImages {
    /// Decoded textures, keyed by URL. `None` means "not an image, or failed".
    cache: HashMap<String, Option<(u32, egui::TextureHandle)>>,
    /// URLs a worker is already fetching, so we never queue the same one twice.
    inflight: HashMap<String, u32>,
    tx: Option<Sender<Msg>>,
}

impl RemoteImages {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self {
            cache: HashMap::new(),
            inflight: HashMap::new(),
            tx: Some(tx),
        }
    }

    /// The texture for `url` sized to `px`, starting a fetch when needed.
    ///
    /// Returns `None` while the fetch is in flight, which is what lets the
    /// caller paint a placeholder for a frame or two.
    pub fn get(&mut self, url: &str, px: u32) -> Option<egui::TextureHandle> {
        let mut stale = None;
        if let Some(hit) = self.cache.get(url) {
            match hit {
                Some((have, tex)) if *have >= px => return Some(tex.clone()),
                Some((_, tex)) => stale = Some(tex.clone()),
                None => return None,
            }
        }
        if self.inflight.get(url).is_some_and(|asked| *asked >= px) {
            return stale;
        }
        let Some(tx) = self.tx.clone() else {
            return stale;
        };
        self.inflight.insert(url.to_owned(), px);
        let url_owned = url.to_owned();
        let spawned = std::thread::Builder::new()
            .name("rhumb-image".into())
            .spawn(move || {
                let (rgba, w, h) = fetch(&url_owned, px).unwrap_or_default();
                let _ = tx.send(Msg::RemoteImage {
                    url: url_owned,
                    px,
                    rgba,
                    w,
                    h,
                });
            });
        if spawned.is_err() {
            self.inflight.remove(url);
        }
        stale
    }

    /// Whether `url` is known not to load, so a caller can stop waiting on it.
    pub fn is_failed(&self, url: &str) -> bool {
        matches!(self.cache.get(url), Some(None))
    }

    /// The texture already fetched for `url`, if any, without starting a fetch.
    /// Used by the preview's layout pass to size an image without fetching it.
    pub fn peek(&self, url: &str) -> Option<egui::TextureHandle> {
        self.cache
            .get(url)
            .and_then(|hit| hit.as_ref())
            .map(|(_, tex)| tex.clone())
    }

    /// Stores a finished fetch, evicting everything when the cache is full.
    pub fn insert(
        &mut self,
        url: String,
        px: u32,
        rgba: Vec<u8>,
        w: u32,
        h: u32,
        ctx: &egui::Context,
    ) {
        self.inflight.remove(&url);
        if rgba.is_empty() || w == 0 || h == 0 {
            if !matches!(self.cache.get(&url), Some(Some(_))) {
                self.cache.insert(url, None);
            }
            return;
        }
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        let tex = ctx.load_texture(format!("remote:{url}"), image, texture_options());
        self.cache.insert(url, Some((px, tex)));
    }
}

fn texture_options() -> egui::TextureOptions {
    egui::TextureOptions {
        magnification: egui::TextureFilter::Linear,
        minification: egui::TextureFilter::Linear,
        wrap_mode: egui::TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    }
}

/// Fetches and decodes an image, returning raw RGBA and its size.
fn fetch(url: &str, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    let mut response = ureq::get(url).call().ok()?;
    let bytes = response.body_mut().read_to_vec().ok()?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return None;
    }
    decode_any(&bytes, px)
}

/// Decodes a raster image, or rasterises an SVG when the bytes are not one.
fn decode_any(bytes: &[u8], px: u32) -> Option<(Vec<u8>, u32, u32)> {
    if let Ok(img) = image::load_from_memory(bytes) {
        return fit_rgba(img.into_rgba8(), px);
    }
    rasterize_svg(bytes, px)
}

/// Shrinks a decoded image to fit a `px` square, never enlarging it.
fn fit_rgba(img: image::RgbaImage, px: u32) -> Option<(Vec<u8>, u32, u32)> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
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

/// Rasterises an SVG to raw RGBA, scaled to fit a `px` square.
fn rasterize_svg(bytes: &[u8], px: u32) -> Option<(Vec<u8>, u32, u32)> {
    let tree = resvg::usvg::Tree::from_data(bytes, &svg_options()).ok()?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    if sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    // A badge is small and stays crisp enlarged a little, so the cap is 2x
    // rather than the "never enlarge" rule used for photographs.
    let scale = (px as f32 / sw.max(sh)).min(2.0);
    let tw = (sw * scale).round().max(1.0) as u32;
    let th = (sh * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(tw, th)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia hands back premultiplied RGBA; egui wants it unmultiplied.
    Some((unpremultiply(pixmap.take()), tw, th))
}

/// Undoes alpha premultiplication, which is what the renderer produces and what
/// egui's `ColorImage` expects not to see.
fn unpremultiply(mut data: Vec<u8>) -> Vec<u8> {
    for px in data.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    data
}

/// The SVG parse options, with the app's own fonts so badge text renders.
fn svg_options() -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        fontdb: fonts().clone(),
        ..Default::default()
    }
}

/// A font database holding the fonts the app already ships.
fn fonts() -> &'static Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_font_data(theme::INTER_REGULAR.to_vec());
        db.load_font_data(theme::INTER_SEMIBOLD.to_vec());
        Arc::new(db)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_decodes_and_shrinks() {
        let src = image::RgbaImage::from_pixel(64, 32, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(src)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (rgba, w, h) = decode_any(&png, 16).expect("decodes");
        assert_eq!((w, h), (16, 8));
        assert_eq!(rgba.len(), 16 * 8 * 4);
    }

    #[test]
    fn an_svg_badge_rasterises() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="20">
            <rect width="80" height="20" rx="3" fill="#7c5cff"/>
            <text x="40" y="14" fill="#fff" font-size="11" text-anchor="middle">2.2</text>
        </svg>"##;
        let (rgba, w, h) = decode_any(svg, 128).expect("rasterises");
        assert!(w >= 80 && h >= 20, "kept the badge's aspect: {w}x{h}");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // The fill is a strong purple, so some pixel must be far from grey.
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0].abs_diff(p[1]) > 20),
            "the badge colour survived"
        );
    }

    #[test]
    fn something_that_is_neither_image_nor_svg_fails_quietly() {
        assert!(decode_any(b"this is not an image", 64).is_none());
    }

    /// `cargo test probe_fetch_badge -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_fetch_badge() {
        let urls = [
            "https://img.shields.io/badge/version-2.2-7c5cff?style=flat-square",
            "https://github.com/user-attachments/assets/36005ffd-9cfc-433e-a306-1606feb18107",
        ];
        for url in urls {
            let out = fetch(url, 512);
            eprintln!(
                "{url}\n  -> {:?}",
                out.as_ref().map(|(rgba, w, h)| (*w, *h, rgba.len()))
            );
            assert!(out.is_some(), "{url} should fetch and decode");
        }
    }

    #[test]
    fn a_cached_texture_is_returned_and_a_failure_is_remembered() {
        let ctx = egui::Context::default();
        // No sender, so nothing is fetched: the cache is exercised directly.
        let mut cache = RemoteImages {
            cache: HashMap::new(),
            inflight: HashMap::new(),
            tx: None,
        };
        let url = "https://example.invalid/a.png";
        assert!(cache.get(url, 64).is_none(), "nothing cached yet");
        cache.insert(url.to_owned(), 64, vec![255, 0, 0, 255], 1, 1, &ctx);
        assert!(cache.get(url, 64).is_some(), "the texture comes back");
        // A failed fetch is remembered, so it is not retried every frame.
        let bad = "https://example.invalid/b.png";
        cache.insert(bad.to_owned(), 64, Vec::new(), 0, 0, &ctx);
        assert!(cache.get(bad, 64).is_none());
        assert!(cache.is_failed(bad), "the failure is known");
        assert!(!cache.is_failed(url), "the good one is not marked failed");
    }
}
