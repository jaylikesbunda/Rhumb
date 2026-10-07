//! Reusable painted pieces: icons, file rows, breadcrumbs, status bar.
//!
//! All geometry here comes from `theme::sp` and `theme::col` so every element
//! lands on the same rhythm, and text is shaped once and cached so painting
//! rows never re-lays-out glyphs.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
    WidgetInfo, WidgetType,
    epaint::text::{LayoutJob, TextWrapping},
};

use crate::fs_model::{self, Entry};
use crate::theme::{bold_font, c, fs as tfs, mono_font, sp, ui_font};

/// The interface's pictograms.
///
/// Each one is a glyph of the Phosphor icon font rather than a shape drawn here, so
/// they are the drawings of a designer, they stay crisp at any size, and they take
/// the text colour like any other text does.
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
    /// A small arrow up, marking the column the list is sorted by.
    Sort,
    /// A cross, for closing a tab.
    Close,
    /// A page marked as Markdown.
    Markdown,
    /// An eye, for showing or hiding the live preview.
    Preview,
    /// An arrow that turns back on itself, for soft wrap.
    Wrap,
    /// A floppy disk, for save.
    Save,
    /// A pushpin, for quick access.
    Pin,
    /// Arrows pointing out: fill the window with this.
    Expand,
    /// Arrows pointing in: go back to side by side.
    Shrink,
    /// A circular arrow, for re-reading the folder.
    Refresh,
    /// A closed chain: two things scroll together.
    Link,
    /// A broken chain: they scroll on their own.
    LinkOff,
    /// The window buttons.
    WinMinimize,
    WinMaximize,
    WinRestore,
    WinClose,
    /// An eye, for hidden files shown.
    Eye,
    /// An eye with a line through it, for hidden files not shown.
    EyeSlash,
    /// Folders inside folders, for searching subfolders too.
    Subfolders,
    /// Any other glyph of the icon font, for a menu's items.
    Glyph(&'static str),
    /// Rows with columns, for the details view of a folder.
    ViewDetails,
    /// A bulleted list, for the compact list view.
    ViewList,
    /// A grid of tiles, for the large icons view.
    ViewLarge,
}

/// The font family the icons are drawn from.
pub const ICON_FAMILY: &str = "rhumb-icons";

impl Icon {
    /// The glyph that draws this icon, with `open` telling the chevron which way to point.
    fn glyph(self, open: bool) -> &'static str {
        use egui_phosphor::regular as ph;
        match self {
            Icon::Back => ph::ARROW_LEFT,
            Icon::Forward => ph::ARROW_RIGHT,
            Icon::Up => ph::ARROW_UP,
            Icon::Folder => ph::FOLDER,
            Icon::File => ph::FILE_TEXT,
            Icon::Chevron if open => ph::CARET_DOWN,
            Icon::Chevron => ph::CARET_RIGHT,
            Icon::Search => ph::MAGNIFYING_GLASS,
            Icon::Sidebar => ph::SIDEBAR_SIMPLE,
            Icon::Sort => ph::CARET_UP,
            Icon::Close => ph::X,
            Icon::Markdown => ph::FILE_MD,
            Icon::Preview => ph::EYE,
            Icon::Wrap => ph::ARROW_BEND_DOWN_LEFT,
            Icon::Save => ph::FLOPPY_DISK,
            Icon::Pin => ph::PUSH_PIN,
            Icon::Expand => ph::ARROWS_OUT_SIMPLE,
            Icon::Shrink => ph::ARROWS_IN_SIMPLE,
            Icon::Refresh => ph::ARROW_CLOCKWISE,
            Icon::Link => ph::LINK,
            Icon::LinkOff => ph::LINK_BREAK,
            Icon::WinMinimize => ph::MINUS,
            Icon::WinMaximize => ph::SQUARE,
            Icon::WinRestore => ph::COPY_SIMPLE,
            Icon::WinClose => ph::X,
            Icon::Eye => ph::EYE,
            Icon::EyeSlash => ph::EYE_SLASH,
            Icon::Subfolders => ph::TREE_STRUCTURE,
            Icon::Glyph(g) => g,
            Icon::ViewDetails => ph::TABLE,
            Icon::ViewList => ph::LIST_BULLETS,
            Icon::ViewLarge => ph::SQUARES_FOUR,
        }
    }

    /// Draws the icon centred in `rect`.
    pub fn paint(self, p: &Painter, rect: Rect, color: Color32) {
        self.paint_with(p, rect, color, false)
    }

    /// Draws the icon as large as `rect` allows, where `paint` stops at the size of
    /// an icon in a row: for the folder on a large tile, which has room for more.
    pub fn paint_large(self, p: &Painter, rect: Rect, color: Color32) {
        let s = rect.height().min(rect.width());
        let font = FontId::new(s * 1.15, egui::FontFamily::Name(ICON_FAMILY.into()));
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            self.glyph(false),
            font,
            color,
        );
    }

    /// Draws the icon, with `open` telling the chevron which way to point.
    ///
    /// Only the chevron cares; every other icon draws the same either way.
    pub fn paint_with(self, p: &Painter, rect: Rect, color: Color32, open: bool) {
        let s = sp::ICON.min(rect.height()).min(rect.width());
        // The glyphs are drawn to fill their em, so the font is a little larger than
        // the space it is to fill.
        let font = FontId::new(s * 1.15, egui::FontFamily::Name(ICON_FAMILY.into()));
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            self.glyph(open),
            font,
            color,
        );
    }
}

/// The height of a row in a menu.
pub const MENU_ROW: f32 = 28.0;

/// One item of a menu: an icon, the label, and a shortcut hint at the right, on a
/// row as wide as the menu so the whole of it is the target. `danger` tints it for
/// what cannot be undone.
pub fn menu_item(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    hint: &str,
    danger: bool,
    enabled: bool,
) -> Response {
    let width = ui.available_width().max(160.0);
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(width, MENU_ROW),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let inner = rect.shrink2(Vec2::new(4.0, 1.0));
        if enabled && resp.hovered() {
            painter.rect_filled(inner, CornerRadius::same(4), c::HOVER);
        }
        let base = if !enabled {
            c::TEXT_GHOST
        } else if danger {
            c::DANGER
        } else {
            c::TEXT
        };
        let dim = if !enabled {
            c::TEXT_GHOST
        } else if danger {
            c::DANGER
        } else {
            c::TEXT_DIM
        };
        icon.paint(
            painter,
            Rect::from_center_size(
                Pos2::new(inner.left() + 14.0, rect.center().y),
                Vec2::splat(16.0),
            ),
            dim,
        );
        let g = layout(ui, label.to_owned(), ui_font(tfs::BODY), base);
        galley_at(
            painter,
            Pos2::new(inner.left() + 32.0, rect.center().y - g.size().y * 0.5),
            &g,
            base,
        );
        if !hint.is_empty() {
            let h = layout(ui, hint.to_owned(), ui_font(tfs::SMALL), c::TEXT_GHOST);
            galley_at(
                painter,
                Pos2::new(
                    inner.right() - 10.0 - h.size().x,
                    rect.center().y - h.size().y * 0.5,
                ),
                &h,
                c::TEXT_GHOST,
            );
        }
    }
    // A painted row is invisible to a screen reader until it says what it is.
    // `enabled` is used rather than `ui.is_enabled()` so a greyed-out item is
    // announced as disabled.
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    resp
}

/// A hairline between groups of menu items, inset from both edges.
pub fn menu_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(160.0), 9.0),
        Sense::hover(),
    );
    ui.painter().hline(
        rect.left() + 10.0..=rect.right() - 10.0,
        rect.center().y,
        Stroke::new(1.0, c::DIVIDER),
    );
}

/// A slider for how big things are: a track, and a round handle that is dragged
/// along it or jumped to with a click. `value` runs from 0 to 1. Returns whether it
/// changed.
pub fn size_slider(ui: &mut Ui, rect: Rect, value: &mut f32) -> bool {
    let resp = ui.interact(
        rect,
        ui.id().with(("size-slider", rect.min.x as i32)),
        Sense::click_and_drag(),
    );
    let track = Rect::from_min_max(
        Pos2::new(rect.left() + 8.0, rect.center().y - 1.5),
        Pos2::new(rect.right() - 8.0, rect.center().y + 1.5),
    );
    let mut changed = false;
    if (resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let v = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            changed = true;
        }
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(2), c::BORDER);
        let x = track.left() + track.width() * value.clamp(0.0, 1.0);
        painter.rect_filled(
            Rect::from_min_max(track.min, Pos2::new(x, track.bottom())),
            CornerRadius::same(2),
            c::TEXT_FAINT,
        );
        let hot = resp.hovered() || resp.dragged();
        painter.circle_filled(
            Pos2::new(x, rect.center().y),
            if hot { 7.0 } else { 6.0 },
            if hot { c::ACCENT } else { c::TEXT },
        );
    }
    // A slider, not a button: the role and the value both matter here.
    let v = f64::from(*value);
    resp.widget_info(|| WidgetInfo::slider(ui.is_enabled(), v, "Size of the items in the list"));
    resp.on_hover_text("Size of the items in the list");
    changed
}

/// An on/off switch: a pill with a knob that sits at the end that is chosen.
///
/// `label` is what a screen reader calls it. The pill is painted here, so
/// nothing else would name it, and a switch with no name is a checkbox a reader
/// cannot tell from the next one.
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(38.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if *on {
            c::ACCENT
        } else if resp.hovered() {
            c::SEL
        } else {
            c::BORDER
        };
        painter.rect_filled(rect, CornerRadius::same(10), fill);
        let x = if *on {
            rect.right() - 10.0
        } else {
            rect.left() + 10.0
        };
        painter.circle_filled(
            Pos2::new(x, rect.center().y),
            7.0,
            if *on { c::BG } else { c::TEXT_DIM },
        );
    }
    // A checkbox to a screen reader, with its state: the role alone would leave
    // whether it is on unsaid, and a switch that only says its name is a switch
    // a reader has to click to understand.
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, label));
    resp
}

/// A row of choices of which one is picked, side by side as one control. Returns the
/// index that was clicked, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], picked: usize) -> Option<usize> {
    let mut clicked = None;
    let widths: Vec<f32> = options
        .iter()
        .map(|o| {
            layout(ui, (*o).to_owned(), ui_font(tfs::BODY), c::TEXT)
                .size()
                .x
                + 24.0
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total + 4.0, 28.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), c::CODE_BG);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(1.0, c::BORDER),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + 2.0;
    for (i, (label, w)) in options.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(*w, 24.0));
        let resp = ui.interact(
            seg,
            ui.id().with(("segment", i, rect.min.x as i32)),
            Sense::click(),
        );
        // Each choice is its own named, selected control.
        resp.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), i == picked, *label)
        });
        if i == picked {
            ui.painter().rect_filled(seg, CornerRadius::same(5), c::SEL);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(seg, CornerRadius::same(5), c::HOVER);
        }
        let color = if i == picked {
            c::SEL_TEXT
        } else {
            c::TEXT_DIM
        };
        let g = layout(ui, (*label).to_owned(), ui_font(tfs::BODY), color);
        galley_at(
            ui.painter(),
            Pos2::new(
                seg.center().x - g.size().x * 0.5,
                seg.center().y - g.size().y * 0.5,
            ),
            &g,
            color,
        );
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// A square icon button that stays lit while `on`, for settings that are either
/// showing or not.
pub fn icon_toggle(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(26.0, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if on {
            painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::SEL);
        } else if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        let color = if on {
            c::ACCENT
        } else if resp.hovered() {
            c::TEXT
        } else {
            c::TEXT_DIM
        };
        icon.paint(painter, rect, color);
    }
    let resp = resp.on_hover_text(tip);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), tip));
    resp
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
    let resp = resp.on_hover_text(tip);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), tip));
    resp
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
    let resp = resp.on_hover_text(tip);
    // The visible label is the name; the tooltip is only the hint.
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    resp
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
        // Air between the last column and whatever panel comes next.
        let right = rect.right() - sp::SM - sp::XS;
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
#[allow(clippy::too_many_arguments)]
pub fn paint_tile(
    ui: &Ui,
    entry: &Entry,
    rect: Rect,
    selected: bool,
    hovered: bool,
    name: &Arc<Galley>,
    thumb: Option<&egui::TextureHandle>,
    shell: Option<&egui::TextureHandle>,
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
            if let Some(tex) = shell {
                // The shell's own picture, filling the art box exactly, so the
                // grid keeps the same geometry as the glyph fallback.
                let size = Vec2::splat(box_px);
                egui::Image::new(tex)
                    .fit_to_exact_size(size)
                    .paint_at(ui, Rect::from_center_size(art.center(), size));
            } else {
                let color = if selected {
                    c::SEL_TEXT
                } else if entry.is_dir {
                    c::TEXT
                } else {
                    c::TEXT_FAINT
                };
                // A folder glyph scaled up reads better than a tiny one.
                let icon = Rect::from_center_size(art.center(), Vec2::splat(box_px * 0.7));
                if entry.is_dir {
                    Icon::Folder.paint_large(painter, icon, color);
                } else if crate::archive::kind_of(&entry.path).is_some() {
                    Icon::Glyph(egui_phosphor::regular::FILE_ZIP).paint_large(painter, icon, color);
                } else {
                    Icon::File.paint_large(painter, icon, color);
                }
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
    shell: Option<&egui::TextureHandle>,
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
    if let Some(tex) = shell {
        // The shell's own picture, at the icon slot's size, so the row reads
        // like Explorer without the layout moving.
        egui::Image::new(tex)
            .fit_to_exact_size(Vec2::splat(icon.height()))
            .paint_at(ui, icon);
    } else if entry.is_dir {
        Icon::Folder.paint(painter, icon, icon_color);
    } else if crate::archive::kind_of(&entry.path).is_some() {
        Icon::Glyph(egui_phosphor::regular::FILE_ZIP).paint(painter, icon, icon_color);
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

/// A quiet heading over a group of rows or tiles in the file list.
///
/// Same voice as the sidebar and details section headers: small, upper-case and
/// faint, with a hairline under it so the groups read as blocks.
pub fn group_header(ui: &Ui, rect: Rect, label: &str) {
    let painter = ui.painter();
    let text = layout(ui, label.to_uppercase(), bold_font(10.0), c::TEXT_GHOST);
    galley_at(
        painter,
        Pos2::new(rect.left() + sp::SM, rect.center().y - text.size().y * 0.5),
        &text,
        c::TEXT_GHOST,
    );
    painter.hline(
        rect.left()..=rect.right(),
        rect.max.y - 0.5,
        Stroke::new(1.0, c::DIVIDER),
    );
}

/// A quiet heading above a run of choices in a menu.
pub fn menu_heading(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let g = layout(ui, text.to_owned(), bold_font(10.0), c::TEXT_GHOST);
    galley_at(
        ui.painter(),
        Pos2::new(rect.left() + 4.0, rect.center().y - g.size().y * 0.5),
        &g,
        c::TEXT_GHOST,
    );
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
    // A breadcrumb is a button that navigates; its segment label is its name.
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
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
    fn every_icon_is_a_glyph_the_icon_font_has() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let id = FontId::new(14.0, egui::FontFamily::Name(ICON_FAMILY.into()));
        let all = [
            Icon::Back,
            Icon::Forward,
            Icon::Up,
            Icon::Folder,
            Icon::File,
            Icon::Chevron,
            Icon::Search,
            Icon::Sidebar,
            Icon::Sort,
            Icon::Close,
            Icon::Markdown,
            Icon::Preview,
            Icon::Wrap,
            Icon::Save,
            Icon::Pin,
            Icon::Expand,
            Icon::Shrink,
            Icon::Refresh,
            Icon::Link,
            Icon::LinkOff,
            Icon::WinMinimize,
            Icon::WinMaximize,
            Icon::WinRestore,
            Icon::WinClose,
            Icon::Eye,
            Icon::EyeSlash,
            Icon::Subfolders,
            Icon::ViewDetails,
            Icon::ViewList,
            Icon::ViewLarge,
        ];
        for icon in all {
            for open in [false, true] {
                let glyph = icon.glyph(open);
                let ch = glyph.chars().next().unwrap();
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&id, ch)),
                    "{icon:?} draws {glyph:?}, which the icon font does not have"
                );
            }
        }
    }

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
        // The date column ends with air before the next panel, and the size
        // column sits to its left with a gap.
        assert!((rect.right() - l.date.x - sp::SM - sp::XS).abs() < 0.01);
        assert!(l.date.x - l.size.x > col::DATE * 0.5);
        // The name has room to draw without colliding with the size column.
        assert!(l.name_limit() > l.name.x + 40.0);
    }

    #[test]
    fn a_columnless_layout_still_places_icon_and_name() {
        // List view builds its layout from the cell with both columns
        // dropped. Zero widths must not collapse the icon and name to the
        // panel edge: that painted every row on top of the border.
        let rect = Rect::from_min_size(Pos2::new(12.0, 200.0), vec2(500.0, sp::ROW));
        let l = RowLayout::new(rect, 0.0, 0.0);
        assert!(!l.shows_size() && !l.shows_date());
        assert!(l.icon.left() > rect.left(), "icon at the panel edge");
        assert!(l.name.x > l.icon.right(), "name left of the icon");
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

    #[test]
    fn the_group_and_menu_headings_draw_without_trouble() {
        // These are painted from the list's grouped layout and the filter menu,
        // which no other test opens, so they are drawn here directly.
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_size(vec2(300.0, 200.0));
            menu_heading(ui, "Kind");
            let (rect, _) = ui.allocate_exact_size(vec2(300.0, sp::SECTION), Sense::hover());
            group_header(ui, rect, "Documents");
        });
        out.drop_without_applying_deltas();
    }

    #[test]
    fn a_painted_button_names_itself_for_a_screen_reader() {
        // A control egui did not paint itself is invisible to AccessKit until it
        // is told its role and name. A click is the one moment egui turns the
        // widget info into an observable event, so the test clicks the button and
        // reads what came out: this is what proves the helper attaches the info
        // at all, which no headless assertion on a painted pixel could.
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let pos = Pos2::new(20.0, 20.0);
        let frame = |events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_size(vec2(200.0, 100.0));
                    let _ = icon_button(ui, Icon::Back, "Back");
                },
            )
        };
        // A widget only answers the pointer on the frame after it is drawn.
        let mut out = frame(vec![]);
        out.drop_without_applying_deltas();
        // Press, then release: the release is the click.
        out = frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ]);
        out.drop_without_applying_deltas();
        out = frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]);
        let info = out
            .platform_output
            .events
            .iter()
            .find_map(|e| match e {
                egui::output::OutputEvent::Clicked(info) => Some(info.clone()),
                _ => None,
            })
            .expect("clicking the button emitted no widget info");
        out.drop_without_applying_deltas();
        assert_eq!(info.typ, WidgetType::Button);
        assert_eq!(info.label.as_deref(), Some("Back"));
    }

    #[test]
    fn a_painted_switch_names_itself_and_says_whether_it_is_on() {
        // A switch is painted here, so unlike a real `egui::Checkbox` it is
        // invisible to AccessKit until it is told its role, name and state. A
        // click is the moment egui turns that into an observable event, so the
        // test clicks it and reads what came out.
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        let pos = Pos2::new(20.0, 20.0);
        let frame = |events: Vec<egui::Event>, on: &mut bool| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_size(vec2(200.0, 100.0));
                    let _ = switch(ui, on, "Show hidden files");
                },
            )
        };
        let mut on = false;
        let mut out = frame(vec![], &mut on);
        out.drop_without_applying_deltas();
        // Press, then release: the release is the click.
        out = frame(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            &mut on,
        );
        out.drop_without_applying_deltas();
        out = frame(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            &mut on,
        );
        let info = out
            .platform_output
            .events
            .iter()
            .find_map(|e| match e {
                egui::output::OutputEvent::Clicked(info) => Some(info.clone()),
                _ => None,
            })
            .expect("clicking the switch emitted no widget info");
        out.drop_without_applying_deltas();
        assert_eq!(info.typ, WidgetType::Checkbox);
        assert_eq!(info.label.as_deref(), Some("Show hidden files"));
        // The state is the one the click just chose.
        assert_eq!(info.selected, Some(true));
    }
}
