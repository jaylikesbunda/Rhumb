//! Reusable painted pieces: icons, file rows, breadcrumbs, status bar.
//!
//! All geometry here comes from `theme::sp` and `theme::col` so every element
//! lands on the same rhythm, and text is shaped once and cached so painting
//! rows never re-lays-out glyphs.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
    epaint::{
        PathStroke,
        text::{LayoutJob, TextWrapping},
    },
};

use crate::fs_model::{self, Entry};
use crate::theme::{bold_font, c, fs as tfs, mono_font, sp, ui_font};

/// Monochrome line icons, drawn rather than bitmapped so they stay crisp at any
/// size and always match the current text colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Back,
    Forward,
    Up,
    Folder,
    File,
    Chevron,
    Search,
    Sidebar,
    Sort,
    /// A cross, for closing a tab.
    Close,
    /// A page with a folded corner and a mark, for Markdown.
    Markdown,
    /// Two panes side by side, for the editor/preview split.
    Preview,
    /// An arrow turning back on itself, for soft wrap.
    Wrap,
    /// An arrow down into a tray, for save.
    Save,
    /// A pushpin, for quick access.
    Pin,
}

impl Icon {
    /// Draws the icon centred in `rect`.
    pub fn paint(self, p: &Painter, rect: Rect, color: Color32) {
        self.paint_with(p, rect, color, false)
    }

    /// Draws the icon, with `open` telling the chevron which way to point.
    ///
    /// Only the chevron cares; every other icon draws the same either way.
    pub fn paint_with(self, p: &Painter, rect: Rect, color: Color32, open: bool) {
        let s = sp::ICON.min(rect.height()).min(rect.width());
        let r = Rect::from_center_size(rect.center(), Vec2::splat(s));
        let stroke = Stroke::new(1.3, color);
        let path_stroke = PathStroke::new(1.3, color);
        let ctr = r.center();

        match self {
            Icon::Back | Icon::Forward => {
                let dir: f32 = if self == Icon::Back { -1.0 } else { 1.0 };
                let h = s * 0.32;
                p.line_segment(
                    [
                        Pos2::new(ctr.x + dir * h * 0.6, ctr.y - h),
                        Pos2::new(ctr.x - dir * h * 0.6, ctr.y),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x - dir * h * 0.6, ctr.y),
                        Pos2::new(ctr.x + dir * h * 0.6, ctr.y + h),
                    ],
                    stroke,
                );
            }
            Icon::Up => {
                let h = s * 0.34;
                p.line_segment(
                    [Pos2::new(ctr.x, ctr.y - h), Pos2::new(ctr.x - h, ctr.y)],
                    stroke,
                );
                p.line_segment(
                    [Pos2::new(ctr.x - h, ctr.y), Pos2::new(ctr.x, ctr.y + h)],
                    stroke,
                );
                p.line_segment(
                    [Pos2::new(ctr.x, ctr.y + h), Pos2::new(ctr.x + h, ctr.y)],
                    stroke,
                );
            }
            Icon::Chevron => {
                // The glyph points right when closed and turns to point down
                // when the folder is open, which is what makes the tree read.
                let h = s * 0.26;
                let a = Pos2::new(ctr.x - h * 0.6, ctr.y - h);
                let b = Pos2::new(ctr.x + h * 0.6, ctr.y);
                let c = Pos2::new(ctr.x - h * 0.6, ctr.y + h);
                let (a, b, c) = if open {
                    // A quarter turn clockwise about the centre.
                    let turn = |p: Pos2| {
                        let d = p - ctr;
                        Pos2::new(ctr.x - d.y, ctr.y + d.x)
                    };
                    (turn(a), turn(b), turn(c))
                } else {
                    (a, b, c)
                };
                p.line_segment([a, b], stroke);
                p.line_segment([b, c], stroke);
            }
            Icon::Folder => {
                // A folder outline: left edge, tab, top edge, right edge, base.
                let pad = s * 0.12;
                let body = Rect::from_min_max(
                    r.min + Vec2::new(pad, pad * 1.5),
                    r.max - Vec2::new(pad, pad * 1.5),
                );
                let tab_w = body.width() * 0.36;
                let tab_h = body.height() * 0.26;
                let p1 = body.left_top();
                let p2 = Pos2::new(p1.x + tab_w * 0.7, p1.y);
                let p3 = Pos2::new(p1.x + tab_w, p1.y + tab_h);
                let p4 = body.right_top() + Vec2::new(0.0, tab_h);
                let p5 = body.right_bottom();
                let p6 = body.left_bottom();
                p.add(Shape::closed_line(
                    vec![p1, p2, p3, p4, p5, p6, p1],
                    path_stroke,
                ));
            }
            Icon::File => {
                // A page with a folded corner.
                let pad = s * 0.18;
                let body =
                    Rect::from_min_max(r.min + Vec2::new(pad, 0.0), r.max - Vec2::new(pad, 0.0));
                let fold = body.width() * 0.42;
                let p1 = body.left_top();
                let p2 = Pos2::new(body.right() - fold, p1.y);
                let p3 = body.right_top();
                let p4 = body.right_bottom();
                let p5 = body.left_bottom();
                p.add(Shape::closed_line(
                    vec![p1, p2, p3, p4, p5, p1],
                    PathStroke::new(1.2, color),
                ));
                p.line_segment([p2, Pos2::new(p2.x, p2.y + fold)], Stroke::new(1.0, color));
                p.line_segment([Pos2::new(p2.x, p2.y + fold), p3], Stroke::new(1.0, color));
            }
            Icon::Search => {
                let rad = s * 0.3;
                let c0 = Pos2::new(ctr.x - s * 0.08, ctr.y - s * 0.08);
                p.circle_stroke(c0, rad, stroke);
                p.line_segment(
                    [
                        c0 + Vec2::new(rad * 0.72, rad * 0.72),
                        c0 + Vec2::new(rad * 1.55, rad * 1.55),
                    ],
                    stroke,
                );
            }
            Icon::Sidebar => {
                let pad = s * 0.14;
                let body =
                    Rect::from_min_max(r.min + Vec2::new(pad, pad), r.max - Vec2::new(pad, pad));
                p.rect_stroke(body, 2.0, stroke, StrokeKind::Inside);
                let x = body.left() + body.width() * 0.36;
                p.line_segment(
                    [Pos2::new(x, body.top()), Pos2::new(x, body.bottom())],
                    stroke,
                );
            }
            Icon::Sort => {
                let h = s * 0.3;
                p.add(Shape::convex_polygon(
                    vec![
                        ctr + Vec2::new(0.0, -h),
                        ctr + Vec2::new(h * 0.8, 0.0),
                        ctr + Vec2::new(-h * 0.8, 0.0),
                    ],
                    color,
                    PathStroke::new(1.2, color),
                ));
            }
            Icon::Close => {
                let h = s * 0.26;
                let stroke = Stroke::new(1.2, color);
                p.line_segment([ctr + Vec2::new(-h, -h), ctr + Vec2::new(h, h)], stroke);
                p.line_segment([ctr + Vec2::new(h, -h), ctr + Vec2::new(-h, h)], stroke);
            }
            Icon::Markdown => {
                // A page outline with two short strokes inside: enough to read
                // as "document" at 14px without needing a typeface.
                let page = Rect::from_center_size(ctr, Vec2::splat(s * 0.86));
                let stroke = Stroke::new(1.1, color);
                p.rect_stroke(page, 2.0, stroke, StrokeKind::Inside);
                let x = page.left() + page.width() * 0.36;
                p.line_segment(
                    [
                        Pos2::new(x, page.top() + 3.0),
                        Pos2::new(x, page.bottom() - 3.0),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(x + 2.5, page.center().y - 2.5),
                        Pos2::new(page.right() - 3.0, page.center().y + 2.5),
                    ],
                    stroke,
                );
            }
            Icon::Preview => {
                // A page with a narrow pane beside it: the split view.
                let h = s * 0.42;
                let stroke = Stroke::new(1.1, color);
                let left = Rect::from_min_max(
                    Pos2::new(ctr.x - h, ctr.y - h),
                    Pos2::new(ctr.x - h * 0.25, ctr.y + h),
                );
                let right = Rect::from_min_max(
                    Pos2::new(ctr.x + h * 0.25, ctr.y - h),
                    Pos2::new(ctr.x + h, ctr.y + h),
                );
                p.rect_stroke(left, 1.5, stroke, StrokeKind::Inside);
                p.rect_stroke(right, 1.5, stroke, StrokeKind::Inside);
            }
            Icon::Wrap => {
                // An arrow that runs right, turns down and doubles back left.
                let h = s * 0.34;
                let stroke = Stroke::new(1.2, color);
                p.line_segment(
                    [
                        Pos2::new(ctr.x - h, ctr.y - h),
                        Pos2::new(ctr.x + h, ctr.y - h),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x + h, ctr.y - h),
                        Pos2::new(ctr.x + h, ctr.y + h * 0.45),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x + h, ctr.y + h * 0.45),
                        Pos2::new(ctr.x - h * 0.2, ctr.y + h * 0.45),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x - h * 0.7, ctr.y - h * 0.05),
                        Pos2::new(ctr.x - h * 0.2, ctr.y + h * 0.45),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x - h * 0.7, ctr.y + h * 0.95),
                        Pos2::new(ctr.x - h * 0.2, ctr.y + h * 0.45),
                    ],
                    stroke,
                );
            }
            Icon::Pin => {
                // A pushpin: a flat head tapering to a point, needle below.
                let h = s * 0.32;
                let w = s * 0.40;
                let stroke = Stroke::new(1.15, color);
                p.line_segment(
                    [
                        Pos2::new(ctr.x - w, ctr.y - h),
                        Pos2::new(ctr.x + w, ctr.y - h),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x - w, ctr.y - h),
                        Pos2::new(ctr.x, ctr.y + h * 0.3),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x + w, ctr.y - h),
                        Pos2::new(ctr.x, ctr.y + h * 0.3),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x, ctr.y + h * 0.3),
                        Pos2::new(ctr.x, ctr.y + h * 1.15),
                    ],
                    stroke,
                );
            }
            Icon::Save => {
                let h = s * 0.34;
                let stroke = Stroke::new(1.2, color);
                p.line_segment(
                    [
                        Pos2::new(ctr.x, ctr.y - h),
                        Pos2::new(ctr.x, ctr.y + h * 0.25),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x - h * 0.55, ctr.y - h * 0.3),
                        Pos2::new(ctr.x, ctr.y + h * 0.25),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x + h * 0.55, ctr.y - h * 0.3),
                        Pos2::new(ctr.x, ctr.y + h * 0.25),
                    ],
                    stroke,
                );
                p.line_segment(
                    [
                        Pos2::new(ctr.x - h, ctr.y + h),
                        Pos2::new(ctr.x + h, ctr.y + h),
                    ],
                    stroke,
                );
            }
        }
    }
}

/// A square icon button with no chrome until hovered.
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(26.0, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        let color = if resp.hovered() { c::TEXT } else { c::TEXT_DIM };
        icon.paint(painter, rect, color);
    }
    resp.on_hover_text(tip)
}

/// A compact flat text button.
pub fn flat_button(ui: &mut Ui, label: &str, tip: &str) -> Response {
    let galley = ui.painter().layout(
        label.to_owned(),
        ui_font(tfs::BODY),
        c::TEXT_DIM,
        f32::INFINITY,
    );
    let pad = Vec2::new(sp::SM, 4.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        galley_at(
            painter,
            rect.min + pad,
            &galley,
            if resp.hovered() { c::TEXT } else { c::TEXT_DIM },
        );
    }
    resp.on_hover_text(tip)
}

/// Column geometry for the file list, so every row lines up exactly.
#[derive(Clone, Copy)]
pub struct RowLayout {
    pub icon: Rect,
    pub name: Pos2,
    pub size: Pos2,
    pub date: Pos2,
    /// The widths the two right-hand columns ended up with. Zero means the
    /// column was dropped because the pane was too narrow to hold it.
    size_w: f32,
    date_w: f32,
}

impl Default for RowLayout {
    /// Empty geometry, for the views that have no columns.
    fn default() -> RowLayout {
        RowLayout {
            icon: Rect::ZERO,
            name: Pos2::ZERO,
            size: Pos2::ZERO,
            date: Pos2::ZERO,
            size_w: 0.0,
            date_w: 0.0,
        }
    }
}

impl RowLayout {
    /// Column geometry for `rect`, with the two right-aligned columns at the
    /// widths the caller resolved for this pane.
    pub fn new(rect: Rect, col_size: f32, col_date: f32) -> RowLayout {
        let icon = Rect::from_min_size(
            Pos2::new(rect.left() + sp::SM, rect.center().y - sp::ICON * 0.5),
            Vec2::splat(sp::ICON),
        );
        let right = rect.right() - sp::SM;
        // A dropped date column hands its space to the size column.
        let size_right = if col_date > 0.0 {
            right - sp::MD - col_date
        } else {
            right
        };
        let name_x = icon.right() + sp::SM;
        RowLayout {
            icon,
            name: Pos2::new(name_x, rect.center().y),
            size: Pos2::new(size_right, rect.center().y),
            date: Pos2::new(right, rect.center().y),
            size_w: col_size,
            date_w: col_date,
        }
    }

    /// Whether the date column has room and should be drawn.
    pub fn shows_date(&self) -> bool {
        self.date_w > 0.0
    }

    /// Whether the size column has room and should be drawn.
    pub fn shows_size(&self) -> bool {
        self.size_w > 0.0
    }

    /// Rightmost x the name may occupy.
    pub fn name_limit(&self) -> f32 {
        self.size_left() - sp::MD
    }

    /// The left edge of the size column, which is where its divider sits.
    pub fn size_left(&self) -> f32 {
        self.size.x - self.size_w
    }

    /// The left edge of the date column, which is where its divider sits.
    pub fn date_left(&self) -> f32 {
        self.date.x - self.date_w
    }
}

/// Pre-shaped text for one row, so painting never re-lays-out text.
pub struct RowGalleys {
    pub name: Arc<Galley>,
    pub size: Arc<Galley>,
    pub date: Arc<Galley>,
}

impl RowGalleys {
    pub fn new(ui: &Ui, entry: &Entry, max_width: f32, font: FontId) -> RowGalleys {
        // A trailing arrow marks a symlink without introducing another colour.
        let name = if entry.is_symlink {
            format!("{} \u{2192}", entry.name)
        } else {
            entry.name.clone()
        };
        RowGalleys {
            name: layout_elided(ui, name, font, c::TEXT, max_width),
            size: layout(
                ui,
                if entry.is_dir {
                    String::new()
                } else {
                    fs_model::fmt_size(entry.size)
                },
                mono_font(tfs::SMALL),
                c::TEXT_FAINT,
            ),
            date: layout(
                ui,
                entry.modified.map_or_else(String::new, fs_model::fmt_time),
                ui_font(tfs::SMALL),
                c::TEXT_FAINT,
            ),
        }
    }
}

/// How the file list is laid out, matching Explorer's three main views.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ViewMode {
    /// Columns with a sortable header: name, size, date.
    #[default]
    Details,
    /// One compact line per item: icon and name only.
    List,
    /// A grid of tiles with a thumbnail or glyph above the name.
    Large,
}

impl ViewMode {
    /// The order the status-bar toggle cycles through.
    pub const ALL: [ViewMode; 3] = [ViewMode::Details, ViewMode::List, ViewMode::Large];

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Details => "Details",
            ViewMode::List => "List",
            ViewMode::Large => "Large icons",
        }
    }

    /// One-line description for the hover tooltip.
    pub fn hint(self) -> &'static str {
        match self {
            ViewMode::Details => "Details view: columns you can sort by",
            ViewMode::List => "List view: one line per item",
            ViewMode::Large => "Large icons view: thumbnails in a grid",
        }
    }

    /// The next mode in the cycle.
    pub fn next(self) -> ViewMode {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// Ctrl+1..3 pick a view directly.
    pub fn from_digit(d: u8) -> Option<ViewMode> {
        match d {
            1 => Some(ViewMode::Details),
            2 => Some(ViewMode::List),
            3 => Some(ViewMode::Large),
            _ => None,
        }
    }

    /// Whether the view draws a grid of tiles rather than full-width rows.
    pub fn is_grid(self) -> bool {
        matches!(self, ViewMode::Large)
    }

    /// Whether size and date columns are shown.
    pub fn has_columns(self) -> bool {
        matches!(self, ViewMode::Details)
    }
}

/// Cache of pre-shaped row text, keyed by row index.
///
/// The cache is dropped whenever the listing or the pane width changes, which
/// keeps scrolling a very large folder close to free.
#[derive(Default)]
pub struct RowCache {
    width: f32,
    rows: std::collections::HashMap<usize, RowGalleys>,
    /// Tile names are shaped at a different width, so they get their own map.
    tiles: std::collections::HashMap<usize, Arc<Galley>>,
}

impl RowCache {
    pub fn clear(&mut self) {
        self.rows.clear();
        self.tiles.clear();
    }

    pub fn get_or_build(&mut self, ui: &Ui, i: usize, entry: &Entry, width: f32) -> &RowGalleys {
        if (self.width - width).abs() > 0.5 {
            self.rows.clear();
            self.width = width;
        }
        self.rows
            .entry(i)
            .or_insert_with(|| RowGalleys::new(ui, entry, width, ui_font(tfs::BODY)))
    }

    /// The centred, elided name under a tile.
    pub fn tile_name(&mut self, ui: &Ui, i: usize, entry: &Entry, width: f32) -> Arc<Galley> {
        self.tiles
            .entry(i)
            .or_insert_with(|| {
                layout_elided(ui, entry.name.clone(), ui_font(tfs::SMALL), c::TEXT, width)
            })
            .clone()
    }
}

/// Draws one tile of the large-icon view: a thumbnail or glyph, then the name.
pub fn paint_tile(
    ui: &Ui,
    entry: &Entry,
    rect: Rect,
    selected: bool,
    hovered: bool,
    name: &Arc<Galley>,
    thumb: Option<&egui::TextureHandle>,
) {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::SEL);
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left() + 1.0, rect.top() + 4.0),
                Vec2::new(2.0, rect.height() - 8.0),
            ),
            1.0,
            c::ACCENT,
        );
    } else if hovered {
        painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::HOVER);
    }

    let pad = sp::SM;
    let text_h = name.size().y;
    let art = Rect::from_min_size(
        Pos2::new(rect.left(), rect.top() + pad),
        Vec2::new(rect.width(), rect.height() - pad * 2.0 - text_h),
    );
    let box_px = art.height().min(art.width() - pad * 2.0).max(16.0);
    let art = Rect::from_center_size(art.center(), Vec2::splat(box_px));

    match thumb {
        Some(tex) => {
            // Keep the aspect ratio inside the square rather than stretching.
            let src = tex.size_vec2();
            let scale = box_px / src.x.max(src.y);
            let size = src * scale;
            let target = Rect::from_center_size(art.center(), size);
            egui::Image::new(tex)
                .fit_to_exact_size(size)
                .paint_at(ui, target);
        }
        None => {
            let color = if selected {
                c::SEL_TEXT
            } else if entry.is_dir {
                c::TEXT
            } else {
                c::TEXT_FAINT
            };
            // A folder glyph scaled up reads better than a tiny one.
            let icon = Rect::from_center_size(art.center(), Vec2::splat(box_px * 0.62));
            if entry.is_dir {
                Icon::Folder.paint(painter, icon, color);
            } else {
                Icon::File.paint(painter, icon, color);
            }
        }
    }

    // Centred name, on one line, elided to the tile.
    let w = name.size().x;
    galley_at(
        painter,
        Pos2::new(rect.center().x - w * 0.5, rect.bottom() - pad - text_h),
        name,
        if selected { c::SEL_TEXT } else { c::TEXT },
    );
}

/// Draws a file row: background, accent bar, icon and the columns.
///
/// `cols` supplies the column x positions; the row's own rect supplies the y,
/// so every line lands on the same rhythm regardless of which row it is.
/// `columns` is false in list view, where only the name is drawn.
#[allow(clippy::too_many_arguments)]
pub fn paint_row(
    ui: &Ui,
    entry: &Entry,
    cols: &RowLayout,
    rect: Rect,
    selected: bool,
    hovered: bool,
    galleys: &RowGalleys,
    columns: bool,
) {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(3), c::SEL);
        // The 2px bar keeps selection unambiguous in a monochrome interface.
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.left() + 1.0, rect.top() + 3.0),
                Vec2::new(2.0, rect.height() - 6.0),
            ),
            1.0,
            c::ACCENT,
        );
    } else if hovered {
        painter.rect_filled(rect, CornerRadius::same(3), c::HOVER);
    }

    let cy = rect.center().y;
    let icon = Rect::from_center_size(
        Pos2::new(cols.icon.center().x, cy),
        Vec2::splat(cols.icon.height()),
    );
    let icon_color = if selected {
        c::SEL_TEXT
    } else if entry.is_dir {
        c::TEXT
    } else {
        c::TEXT_FAINT
    };
    if entry.is_dir {
        Icon::Folder.paint(painter, icon, icon_color);
    } else {
        Icon::File.paint(painter, icon, icon_color);
    }

    let text_color = if selected { c::SEL_TEXT } else { c::TEXT };
    let meta_color = if selected {
        c::SEL_TEXT.gamma_multiply(0.72)
    } else {
        c::TEXT_FAINT
    };

    galley_at(
        painter,
        Pos2::new(cols.name.x, cy - galleys.name.size().y * 0.5),
        &galleys.name,
        text_color,
    );
    if !columns {
        return;
    }
    // A column dropped for want of room is not drawn at all.
    if cols.shows_size() {
        let size_size = galleys.size.size();
        galley_at(
            painter,
            Pos2::new(cols.size.x - size_size.x, cy - size_size.y * 0.5),
            &galleys.size,
            meta_color,
        );
    }
    if cols.shows_date() {
        let date_size = galleys.date.size();
        galley_at(
            painter,
            Pos2::new(cols.date.x - date_size.x, cy - date_size.y * 0.5),
            &galleys.date,
            meta_color,
        );
    }
}

/// Shapes a single-line galley, no wrapping.
pub fn layout(ui: &Ui, text: String, font: FontId, color: Color32) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(&text, 0.0, egui::text::TextFormat::simple(font, color));
    job.wrap = TextWrapping::no_max_width();
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// Shapes text elided in the *middle*, keeping the tail.
///
/// A path is only interesting at the end: "…\Documents\Projects\app" says far
/// more than "C:\Users\…\Documents", so the head is what gets dropped.
pub fn layout_elided_middle(
    ui: &Ui,
    text: String,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let width = max_width.max(10.0);
    let full = layout(ui, text.clone(), font.clone(), color);
    if full.size().x <= width {
        return full;
    }
    // Drop characters from the front until the remainder fits.
    //
    // `n` is how many leading characters go, so the *smallest* `n` that fits is
    // the longest tail worth showing. `fits` is monotone — a shorter tail is
    // never wider — which makes this a lower-bound search. `hi` always holds a
    // candidate known to fit, starting from the bare ellipsis, so the search
    // can never wander off the end and report nothing.
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    let mut best = layout(ui, String::from('\u{2026}'), font.clone(), color);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let candidate: String = std::iter::once('\u{2026}')
            .chain(chars[mid..].iter().copied())
            .collect();
        let g = layout(ui, candidate, font.clone(), color);
        if g.size().x <= width {
            best = g;
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    best
}

/// Shapes text wrapped to `max_width`, keeping every line break the caller put
/// in it. Used where a whole block of text is shown rather than one label.
pub fn layout_wrapped(
    ui: &Ui,
    text: String,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob {
        break_on_newline: true,
        wrap: TextWrapping {
            max_width: max_width.max(20.0),
            // Wrap between any characters so a long path in a preview still fits.
            break_anywhere: true,
            ..Default::default()
        },
        ..Default::default()
    };
    job.append(&text, 0.0, egui::text::TextFormat::simple(font, color));
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// Shapes text, eliding with an ellipsis if it does not fit `max_width`.
pub fn layout_elided(
    ui: &Ui,
    text: String,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(&text, 0.0, egui::text::TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(max_width.max(10.0));
    job.wrap.overflow_character = Some('\u{2026}');
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// The file-list column headers, with the active sort highlighted.
pub fn list_header(
    ui: &Ui,
    rect: Rect,
    sort: fs_model::SortKey,
    ascending: bool,
    col_size: f32,
    col_date: f32,
) {
    let cols = RowLayout::new(rect, col_size, col_date);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, c::PANEL);
    painter.hline(
        rect.left()..=rect.right(),
        rect.max.y - 0.5,
        Stroke::new(1.0, c::BORDER),
    );

    let cy = rect.center().y;
    let paint_col = |text: &str, right: Pos2, align_right: bool, active: bool| {
        let galley = layout(
            ui,
            text.to_owned(),
            if active {
                bold_font(tfs::SMALL)
            } else {
                ui_font(tfs::SMALL)
            },
            if active { c::TEXT } else { c::TEXT_FAINT },
        );
        let size = galley.size();
        let pos = if align_right {
            Pos2::new(right.x - size.x, cy - size.y * 0.5)
        } else {
            Pos2::new(right.x, cy - size.y * 0.5)
        };
        galley_at(
            painter,
            pos,
            &galley,
            if active { c::TEXT } else { c::TEXT_FAINT },
        );
    };

    paint_col(
        fs_model::SortKey::Name.label(),
        cols.name,
        false,
        sort == fs_model::SortKey::Name,
    );
    if cols.shows_size() {
        paint_col(
            fs_model::SortKey::Size.label(),
            cols.size,
            true,
            sort == fs_model::SortKey::Size,
        );
    }
    if cols.shows_date() {
        paint_col(
            fs_model::SortKey::Modified.label(),
            cols.date,
            true,
            sort == fs_model::SortKey::Modified,
        );
    }

    if ascending {
        let x = match sort {
            fs_model::SortKey::Name => cols.name.x + 46.0,
            fs_model::SortKey::Size => cols.size.x - 54.0,
            _ => cols.date.x - 68.0,
        };
        Icon::Sort.paint(
            painter,
            Rect::from_center_size(Pos2::new(x, cy), Vec2::splat(9.0)),
            c::TEXT_DIM,
        );
    }
}

/// A clickable breadcrumb segment.
pub fn breadcrumb_segment(ui: &mut Ui, label: &str, current: bool, max_width: f32) -> Response {
    let color = if current { c::TEXT } else { c::TEXT_DIM };
    let galley = layout_elided(ui, label.to_owned(), ui_font(tfs::BODY), color, max_width);
    let pad = Vec2::new(sp::XS + 1.0, 3.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(3), c::HOVER);
        }
        galley_at(
            painter,
            rect.min + pad,
            &galley,
            if resp.hovered() { c::ACCENT } else { color },
        );
    }
    resp
}

/// The chevron between breadcrumb segments.
pub fn breadcrumb_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(12.0, 18.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        Icon::Chevron.paint(ui.painter(), rect, c::TEXT_GHOST);
    }
}

/// Status bar: item count, selection summary, and optional job progress.
pub fn status_bar(ui: &Ui, rect: Rect, left: &str, center: &str, right: Option<(&str, f32)>) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, c::PANEL);
    painter.hline(
        rect.left()..=rect.right(),
        rect.top() + 0.5,
        Stroke::new(1.0, c::BORDER),
    );

    let cy = rect.center().y;
    let pad = sp::SM;

    let left_galley = layout(ui, left.to_owned(), ui_font(tfs::SMALL), c::TEXT_FAINT);
    let left_x = rect.left() + pad;
    galley_at(
        painter,
        Pos2::new(left_x, cy - left_galley.size().y * 0.5),
        &left_galley,
        c::TEXT_FAINT,
    );

    if !center.is_empty() {
        let available = rect.width() * 0.5;
        let g = layout_elided(
            ui,
            center.to_owned(),
            ui_font(tfs::SMALL),
            c::TEXT_DIM,
            available,
        );
        galley_at(
            painter,
            Pos2::new(
                left_x + left_galley.size().x + sp::MD,
                cy - g.size().y * 0.5,
            ),
            &g,
            c::TEXT_DIM,
        );
    }

    if let Some((text, fraction)) = right {
        let label = format!("{text}  {:.0}%", (fraction.clamp(0.0, 1.0) * 100.0));
        let g = layout(ui, label, ui_font(tfs::SMALL), c::TEXT);
        let x = rect.right() - pad - g.size().x;
        galley_at(painter, Pos2::new(x, cy - g.size().y * 0.5), &g, c::TEXT);

        let track = Rect::from_min_size(Pos2::new(x - 64.0, cy - 1.5), Vec2::new(56.0, 3.0));
        painter.rect_filled(track, 1.5, c::DIVIDER);
        let fill_w = (track.width() * fraction.clamp(0.0, 1.0)).max(1.5);
        painter.rect_filled(
            Rect::from_min_size(track.min, Vec2::new(fill_w, 3.0)),
            1.5,
            c::TEXT_DIM,
        );
    }
}

/// Quiet, centred empty-state text with an optional second line.
pub fn empty_state(ui: &mut Ui, text: &str, sub: &str) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter();
    let g1 = layout(ui, text.to_owned(), ui_font(15.0), c::TEXT_FAINT);
    let g2 = layout(ui, sub.to_owned(), ui_font(tfs::SMALL), c::TEXT_GHOST);
    let total = if sub.is_empty() {
        g1.size().y
    } else {
        g1.size().y + sp::SM + g2.size().y
    };
    let y = rect.center().y - total * 0.5;
    galley_at(
        painter,
        Pos2::new(rect.center().x - g1.size().x * 0.5, y),
        &g1,
        c::TEXT_FAINT,
    );
    if !sub.is_empty() {
        galley_at(
            painter,
            Pos2::new(
                rect.center().x - g2.size().x * 0.5,
                y + g1.size().y + sp::SM,
            ),
            &g2,
            c::TEXT_GHOST,
        );
    }
}

/// Paints a cached galley. `Painter::galley` takes ownership, so the cached
/// `Arc` is unwrapped and copied only for the rows actually on screen.
/// Paints a pre-shaped galley with its top-left corner at `pos`.
///
/// Takes the `Arc` so the shaping cache is shared, never copied.
pub fn galley_at(painter: &Painter, pos: Pos2, galley: &Arc<Galley>, color: Color32) {
    painter.galley(pos, galley.clone(), color);
}

/// Paints `galley` with its right edge at `right`, vertically centred on it.
pub fn text_right(painter: &Painter, right: Pos2, galley: &Arc<Galley>, color: Color32) {
    let size = galley.size();
    painter.galley(
        Pos2::new(right.x - size.x, right.y - size.y * 0.5),
        galley.clone(),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::col;
    use egui::vec2;

    #[test]
    fn middle_elision_keeps_the_tail_of_a_long_path() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let font = ui_font(tfs::BODY);
        let path = String::from(r"C:\Users\somebody\Documents\Projects\app");

        // Narrow: the head must go, the folder name must stay.
        let (narrow, plain, wide) = {
            let (mut n, mut p, mut w) = (None, None, None);
            let path = path.clone();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_max_size(vec2(400.0, 600.0));
                n = Some(layout_elided_middle(
                    ui,
                    path.clone(),
                    font.clone(),
                    c::TEXT,
                    120.0,
                ));
                p = Some(layout(ui, path.clone(), font.clone(), c::TEXT));
                // Roomy: nothing is dropped, so the path reads in full.
                w = Some(layout_elided_middle(
                    ui,
                    path.clone(),
                    font.clone(),
                    c::TEXT,
                    900.0,
                ));
            });
            out.textures_delta.clear();
            (n.unwrap(), p.unwrap(), w.unwrap())
        };
        assert!(
            narrow.size().x <= 120.0,
            "elided to {}px, over the 120px budget",
            narrow.size().x
        );
        assert!(
            plain.size().x > 120.0,
            "fixture is not long enough to be elided"
        );
        let shown: String = narrow.job.text.chars().collect();
        assert!(shown.starts_with('\u{2026}'), "no ellipsis: {shown:?}");
        assert!(shown.ends_with(r"Projects\app"), "lost the tail: {shown:?}");
        assert_eq!(wide.job.text.chars().collect::<String>(), path);
    }

    #[test]
    fn middle_elision_degrades_to_a_bare_ellipsis() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let mut g = None;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_size(vec2(400.0, 600.0));
            g = Some(layout_elided_middle(
                ui,
                String::from("a-very-long-unbreakable-folder-name"),
                ui_font(tfs::BODY),
                c::TEXT,
                1.0,
            ));
        });
        out.textures_delta.clear();
        assert_eq!(g.unwrap().job.text.chars().collect::<String>(), "\u{2026}");
    }

    #[test]
    fn cycling_visits_every_view_and_returns() {
        let mut v = ViewMode::Details;
        let mut seen = vec![v];
        for _ in 0..2 {
            v = v.next();
            seen.push(v);
        }
        assert_eq!(
            seen,
            vec![ViewMode::Details, ViewMode::List, ViewMode::Large]
        );
        // Three steps come back to the start.
        assert_eq!(v.next(), ViewMode::Details);
    }

    #[test]
    fn ctrl_digits_pick_a_view_directly() {
        assert_eq!(ViewMode::from_digit(1), Some(ViewMode::Details));
        assert_eq!(ViewMode::from_digit(2), Some(ViewMode::List));
        assert_eq!(ViewMode::from_digit(3), Some(ViewMode::Large));
        assert_eq!(ViewMode::from_digit(0), None);
        assert_eq!(ViewMode::from_digit(9), None);
    }

    #[test]
    fn only_details_has_columns_and_only_large_is_a_grid() {
        assert!(ViewMode::Details.has_columns());
        assert!(!ViewMode::List.has_columns());
        assert!(!ViewMode::Large.has_columns());
        assert!(!ViewMode::Details.is_grid());
        assert!(!ViewMode::List.is_grid());
        assert!(ViewMode::Large.is_grid());
    }

    #[test]
    fn every_view_has_a_distinct_label() {
        let mut labels: Vec<&str> = ViewMode::ALL.iter().map(|v| v.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ViewMode::ALL.len(), "labels must differ");
        for v in ViewMode::ALL {
            assert!(!v.hint().is_empty(), "{v:?} needs a tooltip");
        }
    }

    #[test]
    fn columns_are_ordered_and_the_name_stops_before_the_size() {
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), vec2(600.0, 26.0));
        let l = RowLayout::new(rect, col::SIZE, col::DATE);
        // Reading order, left to right.
        assert!(l.icon.left() < l.name.x);
        assert!(l.name.x < l.size.x);
        assert!(l.size.x < l.date.x);
        // The date column is inset by the standard right padding, and the size
        // column sits to its left with a gap.
        assert!((rect.right() - l.date.x - sp::SM).abs() < 0.01);
        assert!(l.date.x - l.size.x > col::DATE * 0.5);
        // The name has room to draw without colliding with the size column.
        assert!(l.name_limit() > l.name.x + 40.0);
    }

    #[test]
    fn the_icon_sits_on_the_row_and_inside_it() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 100.0), vec2(400.0, sp::ROW));
        let l = RowLayout::new(rect, col::SIZE, col::DATE);
        assert!((l.icon.center().y - rect.center().y).abs() < 0.01);
        assert!(l.icon.left() >= rect.left());
        assert!(l.icon.right() < rect.right());
    }

    #[test]
    fn dragging_a_divider_moves_only_its_own_column() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 26.0));
        let base = RowLayout::new(rect, col::SIZE, col::DATE);
        // Widening the size column pushes its left edge left, and takes the
        // name's room with it, while the date column stays where it was.
        let wider = RowLayout::new(rect, col::SIZE + 40.0, col::DATE);
        assert!(wider.size_left() < base.size_left());
        assert!((wider.date_left() - base.date_left()).abs() < 0.01);
        assert!((wider.date.x - base.date.x).abs() < 0.01);
        assert!(wider.name_limit() < base.name_limit());
    }

    #[test]
    fn both_columns_at_their_maximum_would_starve_the_name() {
        // This is why the app clamps a drag: the raw geometry has no idea how
        // much room the name needs, so the limit lives with the caller.
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 26.0));
        let l = RowLayout::new(rect, col::MAX, col::MAX);
        assert!(
            l.name_limit() < l.name.x,
            "the unclamped layout is expected to overrun; the drag limit fixes it"
        );
    }

    /// The app clamps a drag against this, so the name always keeps room.
    const MIN_NAME: f32 = 120.0;

    fn col_limit(l: &RowLayout, other: f32) -> f32 {
        let room = l.date.x - sp::MD - other - sp::MD - l.name.x;
        (room - MIN_NAME).clamp(col::MIN, col::MAX)
    }

    #[test]
    fn the_drag_limit_leaves_the_name_enough_room() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 26.0));
        let other = col::MAX;
        let limit = col_limit(&RowLayout::new(rect, col::SIZE, other), other);
        // At the limit, the name still has its minimum.
        let at_limit = RowLayout::new(rect, limit, other);
        assert!(
            at_limit.name_limit() - at_limit.name.x >= MIN_NAME - 0.01,
            "limit {} starves the name",
            limit
        );
        // One pixel more would break it, so the clamp is tight.
        let over = RowLayout::new(rect, limit + 1.0, other);
        assert!(over.name_limit() - over.name.x < MIN_NAME);
    }

    #[test]
    fn a_narrow_pane_still_allows_a_drag() {
        // A very narrow list: the limit must not fall below the minimum width,
        // or the column would freeze instead of resizing.
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(320.0, 26.0));
        let l = RowLayout::new(rect, col::SIZE, col::DATE);
        assert!(col_limit(&l, col::DATE) >= col::MIN);
    }
}
