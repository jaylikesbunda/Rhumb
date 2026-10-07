//! The application: layout, navigation, selection, file operations and the
//! editor / live-preview pane.

mod dialogs;
mod doc;
mod files;
mod folders;
mod keys;
mod list;
mod nav;
mod settings;
mod sidebar;
mod status;
mod strip;
mod toolbar;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, Context, CornerRadius, Frame, Id, Key, Margin, Modal, Pos2, Rect, Sense,
    Stroke, StrokeKind, TextEdit, Ui, Vec2, ViewportCommand,
};
use notify::{RecursiveMode, Watcher};

use crate::archive;
use crate::codeedit;
use crate::editing;
use crate::editor::{self, Doc, DocKind};
use crate::fs_model::{self, DateFilter, Entry, GroupBy, KindFilter, SizeFilter, SortKey};
use crate::markdown::Preview;
use crate::ops::{self, Clipboard};
use crate::search::{self, Search};
use crate::theme::{self, c, fs as tfs, sp};
use crate::thumbs::{self, Thumbs};
use crate::tree::{self, Tree};
use crate::typeahead::{self, TypeAhead};
use crate::widgets::{self, Icon, RowCache, RowLayout, ViewMode};
use crate::workers::{self, Ids, Job, Msg, OpKind, Outcome};

/// How long to wait after the last keystroke before re-rendering Markdown.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(120);
/// Pause after the last keystroke before a recursive search starts.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(140);
/// Coalescing window for filesystem watch events.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);
/// How long change events are ignored after a listing, so a refresh cannot
/// trigger the next one.
const WATCH_COOLDOWN: Duration = Duration::from_millis(1500);
/// How long a toast stays on screen.
const TOAST_TTL: Duration = Duration::from_secs(4);
/// The width the status bar keeps at its right end for the details switch, the size
/// slider and the three view buttons.
const STATUS_CONTROLS_W: f32 = 236.0;
/// How wide the sidebar starts, wide enough for a drive's name and what is free on it.
const SIDEBAR_DEFAULT: f32 = 256.0;
/// What it used to start at. A saved width of exactly this was never dragged there, so
/// it is moved to the new default rather than kept.
const SIDEBAR_OLD_DEFAULT: f32 = 224.0;
/// Sidebar width bounds.
const SIDEBAR_MIN: f32 = 150.0;
const SIDEBAR_MAX: f32 = 340.0;
/// Default and minimum width of the editor / preview pane.
const DOC_MIN: f32 = 380.0;
const DOC_DEFAULT: f32 = 640.0;
/// The least the file list keeps when the editor panel is dragged wide.
const LIST_MIN: f32 = 240.0;

/// The editor panel's width for a stored width and the room there is for it and
/// the file list together: never under `DOC_MIN`, never so wide the list loses
/// `LIST_MIN`. Clamped for display only, so a window that is briefly small does
/// not permanently shrink the width the reader chose.
fn doc_width(stored: f32, room: f32) -> f32 {
    stored.clamp(DOC_MIN, (room - LIST_MIN).max(DOC_MIN))
}

/// The space between the pin and the folder icon on a Quick access row, in points.
const PIN_GAP: f32 = 6.0;
/// Most folders Quick access will hold, and so the most `pinN=` lines in prefs.
const MAX_PINS: usize = 24;
const ID_TITLE_DRAG: &str = "title_drag";
/// Widget-id salt of the live file list. The parked second list uses a salt of
/// its own, or the two panes' rows, headers and scroll areas would share ids.
const ID_LIST: &str = "file_list";
const ID_LIST_SECOND: &str = "file_list_second";
const ID_COL_SIZE: &str = "col_size";
const ID_COL_DATE: &str = "col_date";
const ID_SEARCH: &str = "search_box";

/// State of the directory being shown.
enum Listing {
    Loading,
    Ready,
    Failed,
}

/// One headed group in the file list: its label and the run of items under it.
///
/// `start` indexes `visible`, and the header is drawn immediately before that
/// item. Groups are only used to lay the list out; the item list itself stays
/// flat so selection, counts and keyboard movement never see a header.
pub(super) struct Group {
    pub label: String,
    pub start: usize,
    pub len: usize,
}

/// What a folder tab remembers about where it was, for when it is come back to: the
/// folder and the way back and forward from it, the search or filter, what was
/// selected, and how far the list was scrolled.
#[derive(Clone, Default)]
struct FolderView {
    cwd: PathBuf,
    history: History,
    filter: String,
    scope: Option<SearchScope>,
    sel: HashSet<PathBuf>,
    cursor: usize,
    anchor: usize,
    scroll: f32,
}

/// The most tabs there can be at once.
const MAX_TABS: usize = 24;

/// A whole folder list parked while the other pane of the dual view is live.
///
/// Only one list lives directly on the app at a time. This is everything the
/// other one needs to be drawn and worked on: the entries it listed, how they
/// are filtered and selected, where it is scrolled, and a row cache of its own.
/// Swapping it in for a frame moves values and never re-lists anything.
struct SecondPane {
    cwd: PathBuf,
    entries: Vec<Entry>,
    view: ViewMode,
    visible: Vec<usize>,
    listing: Listing,
    req: u64,
    history: History,
    cursor: usize,
    sel: HashSet<PathBuf>,
    anchor: usize,
    scroll_to: Option<(usize, Align2)>,
    filter: String,
    scope: SearchScope,
    groups: Vec<Group>,
    row_cache: RowCache,
    list_scroll: f32,
    scroll_restore: Option<f32>,
    restore_anchor: Option<usize>,
    search: Search,
    search_shown: bool,
    search_typed: Option<Instant>,
    typeahead: TypeAhead,
}

impl SecondPane {
    /// A pane about to list `cwd`, laid out the way the live list is.
    fn new(cwd: PathBuf, view: ViewMode) -> SecondPane {
        SecondPane {
            cwd,
            entries: Vec::new(),
            view,
            visible: Vec::new(),
            listing: Listing::Loading,
            req: 0,
            history: History::default(),
            cursor: 0,
            sel: HashSet::new(),
            anchor: 0,
            scroll_to: None,
            filter: String::new(),
            scope: SearchScope::Below,
            groups: Vec::new(),
            row_cache: RowCache::default(),
            list_scroll: 0.0,
            scroll_restore: None,
            restore_anchor: None,
            search: Search::default(),
            search_shown: false,
            search_typed: None,
            typeahead: TypeAhead::default(),
        }
    }
}

/// Gives each tab a number of its own, which stays with it however the strip is
/// rearranged, so that "the folder tab being shown" can be told from its place.
fn next_tab_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One open file, as a tab above the editor.
struct Tab {
    id: u64,
    doc: Doc,
}

impl Tab {
    fn file(path: &Path) -> Tab {
        Tab {
            id: next_tab_id(),
            doc: Doc::placeholder(path),
        }
    }

    fn path(&self) -> &Path {
        &self.doc.path
    }

    fn label(&self) -> String {
        self.doc.file_name()
    }
}

/// The Quick access list: folders the user pinned, in the order they pinned
/// them.
///
/// Kept apart from the app so the rules - no duplicates, a cap, comparing
/// paths the way the filesystem does - can be tested without a window. It
/// derefs to the path list so ordinary indexing and iteration keep working.
#[derive(Default)]
struct Pins(Vec<PathBuf>);

/// Open and close state for the right-click menu.
///
/// egui's popups are fiddly in three specific ways, and each of them bit us
/// once, so the rules live here where they can be tested:
///
/// * A popup is only drawn on the frames its function runs, so the path has to
///   outlive the click that opened it.
/// * Re-asserting "open" every frame makes the menu impossible to dismiss, so
///   opening is a one-shot.
/// * egui decides whether a click closes a popup by asking whether the popup
///   was drawn last frame, and `read_response` falls back to the frame before
///   that. A reused id therefore makes the click that opens the menu look like
///   a click that closes it, so each opening gets a fresh id.
#[derive(Default)]
struct MenuState {
    path: Option<PathBuf>,
    anchor: Option<Pos2>,
    wanted: bool,
    epoch: u64,
}

impl MenuState {
    /// Records a right click. The anchor is captured here rather than read per
    /// frame, or the menu would slide along behind the pointer.
    ///
    /// Returns whether the menu was actually opened: when it is already up for
    /// this folder there is nothing to do, and reopening it would drop any
    /// hover state inside under a new popup id.
    fn open(&mut self, anchor: Option<Pos2>, path: PathBuf) -> bool {
        if self.path.as_deref() == Some(path.as_path()) {
            return false;
        }
        self.path = Some(path);
        self.anchor = anchor;
        self.wanted = true;
        self.epoch += 1;
        true
    }

    fn close(&mut self) {
        self.path = None;
        self.anchor = None;
        self.wanted = false;
    }

    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Where the menu should appear.
    fn anchor(&self) -> Option<Pos2> {
        self.anchor
    }

    /// A fresh popup id, never reused between openings.
    fn id(&self) -> Id {
        Id::new(("ctx_menu", self.epoch))
    }

    /// Consumes the request to open. True once per right click.
    fn take_open(&mut self) -> bool {
        std::mem::take(&mut self.wanted)
    }
}

/// Glyph size in the document header, and the gap between it and the text.
const HEADER_ICON: f32 = 15.0;
const HEADER_GAP: f32 = 6.0;
/// The two header rows: title, then the location beneath it.
const HEADER_TITLE_H: f32 = 28.0;
const HEADER_PATH_H: f32 = 19.0;

/// The two-row panel header, shared by the editor and the details pane.
///
/// Glyph and name centred together as one block on the first row, the
/// location centred beneath them on the second. `actions_w` reserves room for
/// buttons on the title row; pass 0 when there are none.
///
/// Centring the icon and the name *together* matters. Centring the name alone
/// would slide the glyph off to the left, further from the words the longer
/// the name gets, and the pair stops reading as a single title.
#[allow(clippy::too_many_arguments)]
fn panel_header(
    ui: &mut Ui,
    header: Rect,
    title_row: Rect,
    glyph: Icon,
    name: String,
    location: String,
    dirty: bool,
    read_only: bool,
    actions_w: f32,
) {
    let painter = ui.painter();
    painter.hline(
        header.left()..=header.right(),
        header.max.y - 0.5,
        Stroke::new(1.0, c::BORDER),
    );
    let cy = title_row.center().y;
    let title_room = Rect::from_min_max(
        Pos2::new(title_row.left() + sp::SM, title_row.top()),
        Pos2::new(title_row.right() - actions_w - sp::SM, title_row.bottom()),
    );
    let name_g = widgets::layout_elided(
        ui,
        name,
        if dirty {
            theme::bold_font(tfs::BODY)
        } else {
            theme::ui_font(tfs::BODY)
        },
        c::TEXT,
        (title_room.width() - HEADER_ICON - HEADER_GAP - if dirty { 11.0 } else { 0.0 }).max(24.0),
    );
    let group_w = HEADER_ICON + HEADER_GAP + name_g.size().x + if dirty { 11.0 } else { 0.0 };
    let group_x = title_room.center().x - group_w * 0.5;
    let icon = Rect::from_center_size(
        Pos2::new(group_x + HEADER_ICON * 0.5, cy),
        Vec2::splat(HEADER_ICON),
    );
    glyph.paint(
        painter,
        icon,
        if dirty { c::TEXT_DIM } else { c::TEXT_GHOST },
    );
    let name_x = icon.right() + HEADER_GAP;
    widgets::galley_at(
        painter,
        Pos2::new(name_x, cy - name_g.size().y * 0.5),
        &name_g,
        c::TEXT,
    );
    if dirty {
        painter.circle_filled(
            Pos2::new(name_x + name_g.size().x + 6.0, cy),
            2.5,
            c::ACCENT,
        );
    }

    // Path underneath, centred on the same axis and elided in the middle so
    // the folder name survives a narrow panel.
    let path_row = Rect::from_min_max(
        Pos2::new(header.left(), title_row.bottom()),
        Pos2::new(header.right(), header.bottom()),
    );
    let read_only_w = if read_only { 58.0 } else { 0.0 };
    let path_room = (path_row.width() - sp::SM * 2.0 - read_only_w).max(24.0);
    let loc = widgets::layout_elided_middle(
        ui,
        location,
        theme::ui_font(tfs::SMALL),
        c::TEXT_FAINT,
        path_room,
    );
    widgets::galley_at(
        painter,
        Pos2::new(
            path_row.center().x - loc.size().x * 0.5,
            path_row.center().y - loc.size().y * 0.5,
        ),
        &loc,
        c::TEXT_FAINT,
    );
    if read_only {
        let g = widgets::layout(
            ui,
            String::from("read only"),
            theme::ui_font(tfs::SMALL),
            c::DANGER,
        );
        widgets::text_right(
            painter,
            Pos2::new(path_row.right() - sp::SM, path_row.center().y),
            &g,
            c::DANGER,
        );
    }
}

/// Whether a queued search request is old enough to start.
///
/// `None` means nothing is queued. That is the case on almost every frame,
/// because the search is pumped from the frame loop rather than from the
/// keystroke, and a gate that only rejected *young* requests restarted the
/// walk every frame - clearing the results as fast as the worker delivered
/// them, so the list read "Searching" and never showed a hit.
fn search_due(typed: Option<Instant>, debounce: Duration) -> bool {
    typed.is_some_and(|t| t.elapsed() >= debounce)
}

/// Narrowest the editor or the preview may be squeezed to, when the panel can
/// afford it.
const SPLIT_MIN_PANE: f32 = 160.0;

/// Narrowest a pane may be, given how much room there is.
///
/// 160 points is about right for a pane someone is reading code in, and it is
/// what the divider stops at in a wide panel. But it cannot also be a floor in a
/// narrow one: the doc panel sits at its default width - around 380 points -
/// whenever the file list is showing, and two 160-point panes plus the divider is
/// 327 of that, leaving the divider 53 points to travel in the whole panel. A drag
/// of 100 points was clipped to 26 and then stopped, which reads as the divider
/// refusing to move rather than as a limit, and which is what "it just bounces
/// back to its original width" turned out to be.
///
/// So below a width where 160 fits twice over, the floor becomes a share of the
/// panel instead: always a quarter, so the divider can always reach anywhere in
/// the middle half of the panel and a drag goes where the pointer goes. Above that
/// width the floor is the constant, and the divider stops where a readable pane
/// ends rather than where a fraction says.
fn split_min_pane(usable: f32) -> f32 {
    SPLIT_MIN_PANE.min(usable * 0.25)
}

/// Where the editor/preview divider sits, in pixels from the left.
///
/// The stored split is a fraction so the panes scale with the panel, but the
/// limits are in pixels because that is what "too narrow to read" means. In a
/// narrow panel a fraction like 0.2 would fall below the limit, so the clamp
/// has to happen here rather than on the fraction.
/// One row of the settings window: the name and what it does at the left, and the
/// control, which `control` draws, at the right end, with a hairline under the row.
fn setting_row(ui: &mut Ui, title: &str, detail: &str, control: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), Sense::hover());
    let name = widgets::layout(ui, title.to_owned(), theme::ui_font(tfs::BODY), c::TEXT);
    widgets::galley_at(
        ui.painter(),
        Pos2::new(rect.left(), rect.top() + 11.0),
        &name,
        c::TEXT,
    );
    let sub = widgets::layout_elided(
        ui,
        detail.to_owned(),
        theme::ui_font(tfs::SMALL),
        c::TEXT_FAINT,
        rect.width() - 200.0,
    );
    widgets::galley_at(
        ui.painter(),
        Pos2::new(rect.left(), rect.top() + 32.0),
        &sub,
        c::TEXT_FAINT,
    );
    let mut at = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                Pos2::new(rect.right() - 300.0, rect.top()),
                rect.max,
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control(&mut at);
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, c::DIVIDER),
    );
}

fn split_left(usable: f32, split: f32) -> f32 {
    let min = split_min_pane(usable);
    (usable * split).clamp(min, usable - min)
}

/// The fraction that puts the divider `left` pixels in.
///
/// The inverse of [`split_left`]. Writing the fraction straight from a raw
/// pixel count is what made the divider jump: the width on screen had been
/// clamped, but the drag wrote back the unclamped value, so a drag that moved
/// nothing still relocated the split.
fn split_fraction(usable: f32, left: f32) -> f32 {
    let min = split_min_pane(usable);
    (left / usable).clamp(min / usable, 1.0 - min / usable)
}

/// Why a folder could not be pinned.
#[derive(Debug, PartialEq, Eq)]
enum PinError {
    /// Already on the list.
    AlreadyThere,
    /// The list is full.
    Full,
}

impl std::ops::Deref for Pins {
    type Target = Vec<PathBuf>;

    fn deref(&self) -> &Vec<PathBuf> {
        &self.0
    }
}

impl Pins {
    /// Adds a folder, normalising it first so `a\b` and `a\b\` are one entry.
    fn add(&mut self, path: &Path, max: usize) -> Result<(), PinError> {
        let path = fs_model::normalize(path);
        if self.0.contains(&path) {
            return Err(PinError::AlreadyThere);
        }
        if self.0.len() >= max {
            return Err(PinError::Full);
        }
        self.0.push(path);
        Ok(())
    }

    /// Removes a folder. Returns whether it was on the list.
    fn remove(&mut self, path: &Path) -> bool {
        let path = fs_model::normalize(path);
        let before = self.0.len();
        self.0.retain(|p| p != &path);
        self.0.len() != before
    }

    fn contains(&self, path: &Path) -> bool {
        let path = fs_model::normalize(path);
        self.0.contains(&path)
    }
}

/// The open tabs and which one is on screen.
///
/// Kept apart from the app so the rules - what closing does to the focus, how
/// cycling wraps - can be tested without a window. It derefs to the tab list so
/// ordinary indexing and iteration keep working.
#[derive(Default)]
struct Tabs {
    open: Vec<Tab>,
    active: usize,
}

impl std::ops::Deref for Tabs {
    type Target = Vec<Tab>;

    fn deref(&self) -> &Vec<Tab> {
        &self.open
    }
}

impl std::ops::DerefMut for Tabs {
    fn deref_mut(&mut self) -> &mut Vec<Tab> {
        &mut self.open
    }
}

impl Tabs {
    fn active_tab(&self) -> Option<&Tab> {
        self.open.get(self.active)
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.open.get_mut(self.active)
    }

    fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Appends a tab and returns its index, without changing the focus.
    fn push(&mut self, tab: Tab) -> usize {
        self.open.push(tab);
        self.open.len() - 1
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.open.iter().position(|t| t.doc.path == path)
    }

    /// Moves the focus, ignoring an out-of-range index.
    fn focus(&mut self, index: usize) {
        if index < self.open.len() {
            self.active = index;
        }
    }

    /// Moves a tab to a new place in the strip, keeping the focus on the same tab.
    fn move_tab(&mut self, from: usize, to: usize) {
        let n = self.open.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let focused = self.open[self.active].id;
        let tab = self.open.remove(from);
        self.open.insert(to, tab);
        if let Some(i) = self.open.iter().position(|t| t.id == focused) {
            self.active = i;
        }
    }

    /// Cycles to the next or previous tab, wrapping at both ends.
    #[cfg(test)]
    fn cycle(&mut self, back: bool) {
        if self.open.len() < 2 {
            return;
        }
        let n = self.open.len();
        self.active = if back {
            (self.active + n - 1) % n
        } else {
            (self.active + 1) % n
        };
    }

    /// Closes a tab and keeps the focus on a sensible neighbour: the same file
    /// if the closed one was before it, otherwise the new last.
    fn close(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        self.open.remove(index);
        self.active = if self.open.is_empty() {
            0
        } else if index < self.active {
            self.active - 1
        } else {
            self.active.min(self.open.len() - 1)
        };
    }

    /// The first tab with unsaved changes, for the closing prompt.
    fn first_dirty(&self) -> Option<usize> {
        self.open.iter().position(|t| t.doc.dirty())
    }
}

/// One reversible file operation, kept for Ctrl+Z.
#[derive(Clone, Debug)]
enum Undo {
    /// New items: undo by deleting them.
    Created(Vec<PathBuf>),
    /// A rename, reversible by renaming back.
    Renamed { from: PathBuf, to: PathBuf },
    /// A whole batch rename, reversible by renaming every pair back.
    RenamedBatch(Vec<(PathBuf, PathBuf)>),
    /// A move, reversible by moving the destinations home.
    Moved { items: Vec<(PathBuf, PathBuf)> },
    /// Copies, undone by removing the copies.
    Copied { items: Vec<PathBuf> },
}

/// A short-lived message.
struct Toast {
    text: String,
    danger: bool,
    born: Instant,
}

/// What to do about one name that already exists at the destination.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConflictChoice {
    /// Overwrite a file, or merge into a folder that is already there.
    Replace,
    /// Leave the existing item and do not place this one.
    Skip,
    /// Place it under a new name, "name (2)".
    KeepBoth,
}

/// Modal dialogs.
enum Dialog {
    None,
    Rename {
        path: PathBuf,
        name: String,
    },
    /// A pattern applied to a whole selection at once. `start` is held as typed
    /// text so an empty or half-finished number survives across frames.
    BatchRename {
        paths: Vec<PathBuf>,
        pattern: String,
        start: String,
    },
    Create {
        dir: PathBuf,
        name: String,
        folder: bool,
    },
    Path {
        text: String,
    },
    ConfirmDelete {
        paths: Vec<PathBuf>,
    },
    /// A copy or move with at least one name already taken at the destination. The
    /// collisions are resolved before the job starts, so the transfer keeps its
    /// single pre-resolved undo mapping.
    Collision {
        /// Sources that do not collide, with the destination each is already bound to.
        ready: Vec<(PathBuf, PathBuf)>,
        /// Sources whose destination name is taken, still to be resolved.
        conflicts: Vec<PathBuf>,
        dest_dir: PathBuf,
        cut: bool,
        /// Whether the next choice applies to every remaining conflict.
        apply_all: bool,
    },
    /// `close_app` distinguishes quitting from closing the file.
    Unsaved {
        path: PathBuf,
        close_app: bool,
    },
    Properties {
        path: PathBuf,
    },
    Help,
    /// The settings window, on the section that is showing.
    Settings {
        section: usize,
    },
}

/// Back / forward history.
#[derive(Default, Clone)]
struct History {
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
}

impl History {
    fn push(&mut self, from: PathBuf) {
        self.back.push(from);
        if self.back.len() > 128 {
            self.back.remove(0);
        }
        self.forward.clear();
    }
    fn back(&mut self, current: &Path) -> Option<PathBuf> {
        let prev = self.back.pop()?;
        self.forward.push(current.to_path_buf());
        Some(prev)
    }
    fn forward(&mut self, current: &Path) -> Option<PathBuf> {
        let next = self.forward.pop()?;
        self.back.push(current.to_path_buf());
        Some(next)
    }
}

/// A running file operation with its latest progress.
struct ActiveJob {
    job: Job,
    done_items: usize,
    total_items: usize,
    done_bytes: u64,
    total_bytes: u64,
    current: String,
    /// The user's wish, mirrored into the job's atomic the worker watches.
    paused: bool,
}

impl ActiveJob {
    /// Whether pausing this job means anything. Only the transfer worker checks
    /// the flag, so a compress, extract or delete job would show a dead switch.
    fn pausable(&self) -> bool {
        matches!(self.job.kind, OpKind::Copy | OpKind::Move)
    }
}

/// What a click in the list means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClickKind {
    Plain,
    Toggle,
    Range,
    Open,
    /// A right click on a row that is already part of the selection: move the
    /// cursor there but leave the selection whole, so the menu can act on all
    /// of it. Collapsing to the one row would make a multi-rename unreachable
    /// from the menu.
    Context,
}

/// How the search box filters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchScope {
    /// Recursive search from the current folder, as you type.
    Below,
    /// Only the entries already listed.
    Here,
}

/// Sidebar navigation tree.
#[derive(Default)]
struct SidebarTree {
    tree: Tree,
    /// The folder whose children are being read.
    loading: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WinAction {
    Minimize,
    Maximize,
    Close,
}

enum CtxAction {
    /// Take the archive, or what is inside one, out into a folder next to it.
    Extract,
    OpenEditor,
    CopyPath,
    OpenTerminal,
    OpenExternal,
    OpenNewWindow,
    Reveal,
    Copy,
    Cut,
    Rename,
    /// The same rename, over everything that is selected.
    RenameMany,
    /// Make a symbolic link beside the item, pointing at it.
    CreateSymlink,
    /// Make a directory junction beside a folder (Windows only).
    #[cfg(windows)]
    CreateJunction,
    Delete,
    DeleteForever,
    Pin,
    Unpin,
    /// Put a Recycle Bin item back where it came from.
    Restore,
    /// Remove a Recycle Bin item for good, without the ordinary file menu.
    DeleteRecycle,
}

/// Keys captured this frame, resolved after the UI is drawn so that focused
/// text widgets get first refusal.
#[derive(Default)]
struct Keys {
    alt_left: bool,
    alt_right: bool,
    alt_up: bool,
    go_path: bool,
    focus_search: bool,
    refresh: bool,
    toggle_hidden: bool,
    toggle_sidebar: bool,
    close_file: bool,
    new_tab: bool,
    next_tab: bool,
    prev_tab: bool,
    new_file: bool,
    new_folder: bool,
    save: bool,
    help: bool,
    select_all: bool,
    rename: bool,
    delete_forever: bool,
    delete: bool,
    /// The layout picked with Ctrl+1..3, if any.
    view: Option<ViewMode>,
    /// Ctrl+Shift+V cycles the layout.
    cycle_view: bool,
    undo: bool,
    properties: bool,
    new_menu: bool,
    toggle_details: bool,
    /// Ctrl+Shift+D shows or hides the second folder list.
    toggle_dual: bool,
    settings: bool,
    search_go: bool,
    escape: bool,
    /// F11 fills the window with the open file, keeping the sidebar.
    focus_mode: bool,
}

/// The application state.
pub struct Rhumb {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ids: Ids,

    // Location
    cwd: PathBuf,
    entries: Vec<Entry>,
    /// How the list is laid out: columns, compact lines or a tile grid.
    view: ViewMode,
    /// Type-ahead find: letters jump to the next matching row.
    typeahead: TypeAhead,
    /// Widths of the two right-hand columns, dragged in the header.
    col_size: f32,
    col_date: f32,
    /// Set by Alt+F; the toolbar button opens its menu and clears the flag.
    new_menu: bool,
    /// Tab the "unsaved changes" prompt is about, when it came from a tab cross.
    pending_close: Option<usize>,
    /// Whether the details pane is offered when nothing is open.
    details: bool,
    visible: Vec<usize>,
    listing: Listing,
    req: u64,
    history: History,

    // Selection
    cursor: usize,
    sel: HashSet<PathBuf>,
    anchor: usize,
    scroll_to: Option<(usize, Align2)>,

    // Preferences
    sort: SortKey,
    ascending: bool,
    show_hidden: bool,
    filter: String,
    /// How the filtered list is broken into headed groups.
    group_by: GroupBy,
    /// Filters that narrow the list alongside the name box. They combine.
    kind_filter: KindFilter,
    date_filter: DateFilter,
    size_filter: SizeFilter,
    /// The filtered items split into their headed groups, in display order.
    /// Empty when grouping is off.
    groups: Vec<Group>,
    search_focus: bool,
    sidebar: bool,
    sidebar_w: f32,
    /// How big the things in the file list are, from 0 to 1: the tiles of the large
    /// icons view, and the height of a row in the others.
    zoom: f32,
    doc_w: f32,
    split: f32,
    /// The open file fills the window: the file list steps aside, the sidebar
    /// stays. On by default; the header button or F11 brings the list back.
    focus: bool,
    row_cache: RowCache,

    // Recursive search
    search: Search,
    search_shown: bool,
    /// Whether typing searches below the current folder or only in it.
    scope: SearchScope,

    // Document
    /// The open tabs and which one is on screen.
    tabs: Tabs,
    /// The folder tabs along the top. There is always at least one, and the one at
    /// `active_folder` is the one on screen: its state is in the app itself, and the files
    /// open in it are `tabs`. The others are parked in their own entries.
    folders: Vec<folders::FolderTab>,
    active_folder: usize,
    /// How the two rows of tabs are slid and where they were drawn.
    folder_strip: strip::State,
    doc_strip: strip::State,
    /// How far the file list was scrolled on its last frame, in points.
    list_scroll: f32,

    // Dual pane
    /// The second folder list, when the dual view is on and one has been made.
    second: Option<SecondPane>,
    /// Whether the two folder lists are shown side by side.
    dual: bool,
    /// Which side the live pane is drawn on. Kept so a click in the other pane
    /// makes it live without either folder jumping across the screen.
    live_on_left: bool,
    /// Where the divider between the two lists sits, as a fraction of the room.
    dual_split: f32,

    /// Where to scroll the list to once it has rows to scroll, after a tab comes back.
    scroll_restore: Option<f32>,
    /// Where the selection's anchor goes once the listing has arrived, after a tab
    /// comes back: arriving resets it to the cursor, as for any new listing.
    restore_anchor: Option<usize>,
    loading: Option<PathBuf>,
    preview: Preview,
    /// The document text last handed to the preview, so an idle frame does not
    /// copy the whole buffer again.
    preview_buffer: String,
    preview_buffer_version: u64,
    render_version: u64,
    last_edit: Option<Instant>,
    wrap: bool,
    preview_visible: bool,
    /// Whether scrolling the editor scrolls the preview and the other way about.
    sync_scroll: bool,
    /// The preview's scroll position and range on its last frame, in points.
    preview_off: f32,
    preview_range: f32,
    /// Where the editor asked the preview to scroll to, for its next frame.
    preview_set: Option<f32>,
    /// Which pane was scrolled last, and so is the one the other is kept in line with.
    editor_leads: bool,
    /// Where the preview pane was on its last frame, to tell which pane the pointer is in.
    preview_rect: Rect,
    /// Where the editor pane was on its last frame, for the same reason.
    editor_rect: Rect,
    /// The wheel delta as the frame began, before a scroll area consumed it. Used to
    /// tell which pane the reader is scrolling.
    wheel_this_frame: f32,

    // Operations
    clip: Option<Clipboard>,
    jobs: Vec<ActiveJob>,
    /// Most recent undoable action, plus the history behind it.
    undo: Option<Undo>,
    undo_stack: Vec<Undo>,
    /// Action recorded by a worker, moved into the stack when it finishes.
    pending_undo: Option<Undo>,

    // UI state
    dialog: Dialog,
    toasts: Vec<Toast>,
    watcher: Option<Box<dyn Watcher>>,
    watch_target: Option<PathBuf>,
    last_change: Option<Instant>,
    /// When the last listing finished, for the watcher cooldown.
    listed_at: Option<Instant>,
    /// Decoded image thumbnails, filled on demand by the icon view.
    thumbs: Thumbs,
    /// Remote images for the Markdown preview, fetched on demand.
    remote: crate::remote::RemoteImages,
    /// The name indexes searches are answered from.
    indexes: crate::index::Indexes,
    /// A saved index being read on a worker; added to `indexes` when it lands,
    /// so reading a large snapshot does not delay the first frame.
    index_load: Option<std::sync::mpsc::Receiver<crate::index::Index>>,
    /// Whether the window has been shown. It starts hidden so the first frame is
    /// painted before it appears, which is what removes the startup flash.
    shown: bool,
    /// Head of the text file the details pane is showing, with its path.
    peek: Option<(PathBuf, String)>,
    /// Path whose head is being read, so it is only requested once.
    peek_pending: Option<PathBuf>,
    /// Free space per volume, cached: reading it is milliseconds of work.
    free_space: fs_model::FreeSpace,
    roots: fs_model::Roots,
    /// Folder the pointer is over while dragging, if any.
    drop_target: Option<PathBuf>,
    /// The right-click menu: which path, where, and whether to open it.
    menu: MenuState,
    /// Folders the user pinned to Quick access, in the order they pinned them.
    pins: Pins,
    /// Folder sizes and counts, measured on a worker. Walking a folder is far
    /// too slow to do on the frame that draws it.
    measures: ops::Measures,
    /// Paths being dragged inside the app.
    drag_payload: Vec<PathBuf>,
    /// Last keystroke in the search box, used to debounce recursive searches.
    search_typed: Option<Instant>,
    close_armed: bool,
    /// The text editor's own state: caret, selection, scroll and undo.
    ///
    /// One editor is on screen at a time, so this lives here rather than in
    /// egui's widget memory, and is reset whenever the document changes.
    ed: codeedit::Editor,
    /// Whether the editor had focus last frame, which decides who owns the
    /// clipboard shortcuts and the selection keys.
    ed_focused: bool,
    /// The editor took this frame's clipboard shortcut.
    ed_took_clipboard: bool,
    /// Editor timing, for the `RHUMB_BENCH` probe. Read every frame, written
    /// only when the probe is set.
    bench_since: Option<Instant>,
    bench_frames: u32,
    bench_total: f32,
    bench_max: f32,
    /// Milliseconds the periodic housekeeping at the top of a frame took.
    bench_house: f32,
    /// Milliseconds each named part of the frame took, worst seen. Only ever
    /// written when the probe is set; read by nothing but the log line.
    bench_sections: Vec<(&'static str, f32)>,
    /// Explorer-style folder tree in the sidebar.
    sidebar_tree: SidebarTree,
    /// Last known window inner rect, saved on exit.
    window_rect: Option<[f32; 4]>,

    perf: Perf,
}

/// Timing shown in the status bar.
///
/// `update_ms` is the time actually spent inside our update, smoothed. It is
/// deliberately *not* the interval between repaints: egui only repaints when
/// something changes, so that interval would just measure idleness.
#[derive(Default)]
struct Perf {
    update_ms: f32,
    list_ms: f32,
}

/// Whether the `RHUMB_BENCH` probe is switched on.
///
/// Asked eight times a frame, and reading the environment on Windows walks the
/// process's whole environment block - so asking it in the loop made the probe
/// cost more than the thing it was measuring, and every number it reported was
/// inflated by it. Read once.
fn bench_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RHUMB_BENCH").is_some())
}

impl Rhumb {
    /// Builds the app and starts the first directory listing.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Rhumb {
        // What windows that were closed badly left behind.
        let _ = std::thread::Builder::new()
            .name("rhumb-sweep".into())
            .spawn(archive::sweep_cache);
        let (tx, rx) = workers::bus();
        let arg = std::env::args().nth(1).map(PathBuf::from);

        let mut app = Rhumb {
            tx: tx.clone(),
            rx,
            ids: Ids::default(),
            cwd: arg
                .as_ref()
                .filter(|p| p.is_dir())
                .cloned()
                .unwrap_or_else(default_dir),
            entries: Vec::new(),
            view: ViewMode::default(),
            typeahead: TypeAhead::default(),
            col_size: theme::col::SIZE,
            col_date: theme::col::DATE,
            new_menu: false,
            pending_close: None,
            details: true,
            visible: Vec::new(),
            listing: Listing::Loading,
            req: 0,
            history: History::default(),
            cursor: 0,
            sel: HashSet::new(),
            anchor: 0,
            scroll_to: None,
            sort: SortKey::Name,
            ascending: true,
            show_hidden: false,
            filter: String::new(),
            group_by: GroupBy::None,
            kind_filter: KindFilter::All,
            date_filter: DateFilter::Any,
            size_filter: SizeFilter::Any,
            groups: Vec::new(),
            search_focus: false,
            sidebar: true,
            focus: true,
            sidebar_w: SIDEBAR_DEFAULT,
            zoom: 0.25,
            doc_w: DOC_DEFAULT,
            split: 0.5,
            row_cache: RowCache::default(),
            search: Search::default(),
            search_shown: false,
            scope: SearchScope::Below,
            sidebar_tree: SidebarTree::default(),
            tabs: Tabs::default(),
            folders: Vec::new(),
            active_folder: 0,
            folder_strip: strip::State::default(),
            doc_strip: strip::State::default(),
            list_scroll: 0.0,
            second: None,
            dual: false,
            live_on_left: true,
            dual_split: 0.5,
            scroll_restore: None,
            restore_anchor: None,
            loading: None,
            preview: Preview::new(),
            preview_buffer: String::new(),
            // Nothing has been synced yet, so the first frame must copy.
            preview_buffer_version: u64::MAX,
            render_version: 0,
            last_edit: None,
            wrap: false,
            preview_visible: true,
            sync_scroll: true,
            preview_off: 0.0,
            preview_range: 0.0,
            preview_set: None,
            editor_leads: true,
            preview_rect: Rect::NOTHING,
            editor_rect: Rect::NOTHING,
            wheel_this_frame: 0.0,
            clip: None,
            jobs: Vec::new(),
            undo: None,
            undo_stack: Vec::new(),
            pending_undo: None,
            dialog: Dialog::None,
            toasts: Vec::new(),
            watcher: None,
            watch_target: None,
            last_change: None,
            listed_at: None,
            thumbs: Thumbs::new(tx.clone()),
            remote: crate::remote::RemoteImages::new(tx.clone()),
            indexes: crate::index::Indexes::default(),
            index_load: None,
            shown: false,
            peek: None,
            peek_pending: None,
            free_space: fs_model::FreeSpace::default(),
            roots: fs_model::Roots::default(),
            drop_target: None,
            menu: MenuState::default(),
            pins: Pins::default(),
            measures: ops::Measures::new(tx.clone()),
            drag_payload: Vec::new(),
            search_typed: None,
            close_armed: false,
            ed: codeedit::Editor::default(),
            ed_focused: false,
            ed_took_clipboard: false,
            bench_since: None,
            bench_frames: 0,
            bench_total: 0.0,
            bench_max: 0.0,
            bench_house: 0.0,
            bench_sections: Vec::new(),
            window_rect: None,
            perf: Perf::default(),
        };

        if let Some(s) = cc.storage {
            let _ = s;
        }
        cc.egui_ctx.set_fonts(theme::fonts());
        cc.egui_ctx.all_styles_mut(|s| *s = theme::style());
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        app.apply_prefs(arg.as_deref().filter(|p| p.is_dir()));
        // The window starts with one folder tab, for where it opened.
        app.init_folders();
        // Read back the saved index of the folder being shown, on a worker: a
        // large snapshot is a gunzip and a parse, and doing that here would put
        // it in front of the first frame. It is added when it arrives.
        let (itx, irx) = std::sync::mpsc::channel();
        let cwd = app.cwd.clone();
        let _ = std::thread::Builder::new()
            .name("rhumb-index-load".into())
            .spawn(move || {
                if let Some(index) = crate::index::Indexes::load_cached(&cwd) {
                    let _ = itx.send(index);
                }
            });
        app.index_load = Some(irx);
        app.request_listing();
        // A window that was left in dual-pane view opens with its second list
        // reading again, so the two panes start in step.
        if app.dual {
            app.ensure_second();
        }
        if let Some(p) = arg.filter(|p| p.is_file()) {
            app.open_path(&p);
        }
        app
    }

    /// Adds a saved name index a worker finished reading, if one arrived.
    fn take_loaded_index(&mut self) {
        let Some(rx) = self.index_load.take() else {
            return;
        };
        let mut got = Vec::new();
        let mut open = true;
        loop {
            match rx.try_recv() {
                Ok(index) => got.push(index),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        if open {
            self.index_load = Some(rx);
        }
        for index in got {
            self.indexes.add(index);
        }
    }

    // ---- persistence ---------------------------------------------------

    fn draw(&mut self, root: &mut Ui, ctx: &Context) {
        // Timed around the whole update, so the number is work done rather than
        // the gap between repaints (egui only repaints when something changes).
        let started = Instant::now();
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.window_rect = Some([rect.min.x, rect.min.y, rect.width(), rect.height()]);
        }

        self.drain_messages(ctx);
        self.take_loaded_index();
        self.pump_search();
        self.expire_toasts();
        self.handle_watch_debounce();
        // A second launch arrives as a file; `instance::watch` wakes the window for it.
        self.take_instance_signal(ctx);
        self.close_guard(ctx);
        // Split from the drawing below, so a slow frame can be attributed: the
        // work above is periodic housekeeping, the work below is layout and
        // paint, and only one of them is ever the problem.
        let housekeeping = started.elapsed().as_secs_f32() * 1000.0;
        // Each block below is timed, so a slow frame can be attributed to a part
        // of the layout rather than merely counted. The list is a local so that
        // nothing here has to borrow the app, which the panels below all need.
        // With the probe off the macro expands to a no-op and costs nothing.
        let mut section = std::time::Instant::now();
        let mut bench_sections: Vec<(&'static str, f32)> = Vec::new();
        // Restarts the clock. Taken as a function rather than written inline so
        // that the assignment is a read-modify-write of a live binding, which
        // lints accept; an inline `section = Instant::now()` in the last mark of
        // the frame is genuinely dead and is reported as such.
        fn restart(at: &mut std::time::Instant) {
            *at = std::time::Instant::now();
        }
        macro_rules! mark {
            ($name:literal) => {
                if bench_on() {
                    let ms = section.elapsed().as_secs_f32() * 1000.0;
                    match bench_sections.iter_mut().find(|(n, _)| *n == $name) {
                        Some(slot) => slot.1 = slot.1.max(ms),
                        None => bench_sections.push(($name, ms)),
                    }
                    restart(&mut section);
                }
            };
        }

        egui::containers::Panel::top("titlebar")
            .exact_size(sp::TITLE)
            .resizable(false)
            .frame(title_frame())
            .show(root, |ui| self.title_bar(ui));
        mark!("titlebar");

        egui::containers::Panel::top("toolbar")
            .exact_size(sp::TOOLBAR)
            .resizable(false)
            .frame(toolbar_frame())
            .show(root, |ui| self.toolbar(ui));
        mark!("toolbar");

        // The status bar claims the full window width, so it is shown before the
        // side panels: a panel only gets what the earlier ones left behind, and
        // Explorer's status bar runs edge to edge under everything.
        // The tab row spans the whole window, the way Explorer puts it above
        // the folder view and the document pane.
        // The folder tabs, along the top, always: a window is one or more places.
        self.init_folders();
        egui::containers::Panel::top("tabs")
            .exact_size(sp::TAB_H)
            .resizable(false)
            .frame(tab_frame())
            .show(root, |ui| {
                let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                self.folder_strip_ui(ui, rect);
            });
        mark!("tabs");

        egui::containers::Panel::bottom("status")
            .exact_size(sp::STATUS)
            .resizable(false)
            .frame(status_frame())
            .show(root, |ui| self.status_ui(ui));
        mark!("status");

        if self.sidebar {
            let sidebar_w = self.sidebar_w;
            egui::containers::Panel::left("sidebar")
                .resizable(true)
                .default_size(sidebar_w)
                .min_size(SIDEBAR_MIN)
                .max_size(SIDEBAR_MAX)
                .show_separator_line(false)
                .frame(sidebar_frame())
                .show(root, |ui| {
                    let w = ui.max_rect().width();
                    self.sidebar_ui(ui);
                    if w > 0.0 {
                        self.sidebar_w = w;
                    }
                });
        }
        mark!("sidebar");

        // Focus mode fills the window with the open file: the file list steps
        // aside and the editor takes the centre. The sidebar stays where it
        // is. With no file open, or focus off, the usual list applies.
        let focus_mode = self.focus && self.shows_file_tab();

        // The right-hand pane is either the editor or, when nothing is open,
        // the details pane for the current selection. One pane, two jobs.
        if (self.shows_file_tab() || (self.details && !self.sel.is_empty())) && !focus_mode {
            // The width is ours, not egui's. A resizable panel stores whatever
            // width its content ended up needing, so any content with a minimum
            // width quietly overrides a drag on release and the panel springs back
            // to where it was. Here the width is a number this code owns, clamped
            // for display and written only by the handle below.
            let avail = root.available_width();
            let doc_w = doc_width(self.doc_w, avail);
            let shown = egui::containers::Panel::right("doc_panel")
                .resizable(false)
                .exact_size(doc_w)
                .show_separator_line(false)
                .frame(doc_frame())
                .show(root, |ui| {
                    if self.shows_file_tab() {
                        self.doc_ui(ui);
                    } else {
                        self.details_ui(ui);
                    }
                });
            let edge = shown.response.rect;
            let grab = Rect::from_min_max(
                Pos2::new(edge.left() - 3.0, edge.top()),
                Pos2::new(edge.left() + 3.0, edge.bottom()),
            );
            let handle = root.interact(grab, Id::new("doc_resize"), Sense::drag());
            if handle.hovered() || handle.dragged() {
                root.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                root.painter()
                    .rect_filled(grab, CornerRadius::ZERO, c::HOVER);
            }
            if handle.dragged()
                && let Some(p) = handle.interact_pointer_pos()
            {
                self.doc_w = doc_width(edge.right() - p.x, avail);
            }
        }
        mark!("doc panel");

        egui::CentralPanel::default()
            .frame(Frame::central_panel(&theme::style()))
            .show(root, |ui| {
                if focus_mode {
                    self.doc_ui(ui);
                } else if self.dual && self.second.is_some() && !self.shows_file_tab() {
                    // Two folder lists side by side; the toolbar, tabs and
                    // status bar above and below still speak for the live one.
                    self.dual_list_ui(ui);
                } else {
                    self.list_ui(ui, ID_LIST);
                }
            });
        mark!("central");

        // The path outlives the click that opened it: a popup is only drawn on the
        // frames its function runs, so stop asking and it vanishes. Drawn here, over
        // everything, and not from the file list: a menu opened from the sidebar or
        // with the list out of the way was never drawn at all, which is why a pinned
        // folder could not be unpinned.
        if let Some(path) = self.menu.path().map(Path::to_path_buf) {
            self.context_menu(root, &path);
        }
        self.handle_file_drop(ctx);
        self.resize_edges(ctx);
        self.dialogs(ctx);
        self.draw_toasts(ctx);
        self.handle_keys(ctx);

        let done = started.elapsed().as_secs_f32() * 1000.0;
        self.bench_house = housekeeping;
        if bench_on() {
            self.bench_sections = bench_sections;
        }
        self.perf.update_ms = if self.perf.update_ms == 0.0 {
            done
        } else {
            self.perf.update_ms * 0.9 + done * 0.1
        };
    }

    // ---- title bar -------------------------------------------------------
}
/// The Properties sheet, the same information Explorer shows, in one column.
///
/// `measures` is the cached subtree total for a folder, or `None` while a
/// worker is still walking it.
fn properties_ui(ui: &mut Ui, path: &Path, measures: Option<ops::Measure>) {
    let md = std::fs::metadata(path).ok();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(path);

    section_header(ui, "GENERAL");

    // Each row is one allocated strip: the label column is fixed, the value
    // takes the rest and is elided. Painting is done from the allocated rect,
    // so a long value can never push the layout sideways.
    let label_w = 92.0f32;
    let row = |ui: &mut Ui, k: &str, v: String| {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        let key_g = widgets::layout(ui, k.to_owned(), theme::ui_font(tfs::SMALL), c::TEXT_FAINT);
        let val_g = widgets::layout_elided(
            ui,
            v,
            theme::ui_font(tfs::BODY),
            c::TEXT,
            (rect.width() - label_w - sp::SM).max(20.0),
        );
        let cy = rect.center().y;
        let painter = ui.painter();
        widgets::galley_at(
            painter,
            Pos2::new(rect.left() + 2.0, cy - key_g.size().y * 0.5),
            &key_g,
            c::TEXT_FAINT,
        );
        widgets::galley_at(
            painter,
            Pos2::new(rect.left() + label_w, cy - val_g.size().y * 0.5),
            &val_g,
            c::TEXT,
        );
    };

    row(ui, "Name", name);
    row(
        ui,
        "Type",
        match md.as_ref() {
            Some(m) if m.is_dir() => "File folder".to_owned(),
            Some(m) if m.is_symlink() => "Symbolic link".to_owned(),
            _ => kind_of(path),
        },
    );
    row(ui, "Location", parent.display().to_string());
    let is_dir = md.as_ref().is_some_and(std::fs::Metadata::is_dir);
    row(
        ui,
        "Size",
        match (is_dir, measures) {
            // A folder is measured in the background; until the answer lands
            // show a dash rather than walk the tree on this frame.
            (true, Some(m)) => fs_model::fmt_size(m.bytes),
            (true, None) => String::from("-"),
            (false, _) => md
                .as_ref()
                .map_or_else(|| String::from("-"), |m| fs_model::fmt_size(m.len())),
        },
    );
    if let Some(m) = md.as_ref() {
        if is_dir {
            row(
                ui,
                "Contains",
                measures.map_or_else(
                    || String::from("-"),
                    |m| format!("{} files, {} folders", m.files, m.folders),
                ),
            );
        }
        row(
            ui,
            "Created",
            m.created().map_or(String::from("-"), fmt_stamp),
        );
        row(
            ui,
            "Modified",
            m.modified().map_or(String::from("-"), fmt_stamp),
        );
        row(
            ui,
            "Accessed",
            m.accessed().map_or(String::from("-"), fmt_stamp),
        );
    }
    let mut attrs: Vec<&str> = Vec::new();
    if md.as_ref().is_some_and(|m| m.permissions().readonly()) {
        attrs.push("Read-only");
    }
    if path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    {
        attrs.push("Hidden");
    }
    row(
        ui,
        "Attributes",
        if attrs.is_empty() {
            "-".to_owned()
        } else {
            attrs.join(", ")
        },
    );
}

/// A friendly type name from the extension.
fn kind_of(path: &Path) -> String {
    let ext = fs_model::ext_of(path);
    if ext.is_empty() {
        return "File".to_owned();
    }
    if fs_model::is_markdown(path) {
        return "Markdown document".to_owned();
    }
    let name = match ext.as_str() {
        "rs" => "Rust source",
        "py" => "Python script",
        "js" | "mjs" | "cjs" => "JavaScript file",
        "ts" => "TypeScript file",
        "json" => "JSON file",
        "toml" => "TOML file",
        "yaml" | "yml" => "YAML file",
        "txt" | "log" => "Text document",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "svg" => "SVG image",
        "zip" => "Compressed folder",
        "exe" => "Application",
        "dll" => "Extension",
        _ => "",
    };
    if name.is_empty() {
        format!("{ext} file")
    } else {
        name.to_owned()
    }
}

fn fmt_stamp(t: std::time::SystemTime) -> String {
    match jiff::Timestamp::from_second(
        t.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    ) {
        Ok(ts) => {
            let d = ts.to_zoned(jiff::tz::TimeZone::system()).datetime();
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                d.year(),
                d.month(),
                d.day(),
                d.hour(),
                d.minute()
            )
        }
        Err(_) => String::from("-"),
    }
}

fn section_header(ui: &mut Ui, title: &str) {
    let g = widgets::layout(ui, title.to_owned(), theme::bold_font(10.0), c::TEXT_GHOST);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), sp::SECTION), Sense::hover());
    // The allocated rect, not `max_rect`, so headers never stack on one spot.
    let size = g.size();
    widgets::galley_at(
        ui.painter(),
        Pos2::new(rect.left() + 2.0, rect.center().y - size.y * 0.5),
        &g,
        c::TEXT_GHOST,
    );
}

/// The narrowest the name column is ever allowed to become.
const MIN_NAME_COL: f32 = 120.0;
/// The narrowest a column can be before its own text would collide with the
/// one beside it. Both are sized from the widest value each column ever shows.
const MIN_SIZE_COL: f32 = 60.0;
const MIN_DATE_COL: f32 = 84.0;

/// Resolves the two right-hand column widths for a pane of `width`.
///
/// A dragged width is a preference, not a promise: in a narrow pane the columns
/// give way to the name, and the date goes first because it is the least useful
/// one to have. A dropped column comes back as soon as there is room again.
fn fit_columns(width: f32, want_size: f32, want_date: f32) -> (f32, f32) {
    // The icon, its gap, the padding in front of the columns and the right
    // margin: everything the columns cannot have.
    let chrome = sp::SM + sp::ICON + sp::SM + sp::MD + sp::MD + sp::SM;
    let budget = (width - chrome - MIN_NAME_COL).max(0.0);
    let wanted = want_size + want_date;
    if wanted <= budget {
        return (want_size, want_date);
    }
    // Take from the date column first, then the size column, keeping whatever
    // stays wide enough to hold its own text.
    let mut excess = wanted - budget;
    let mut date = want_date;
    let take = excess.min((date - MIN_DATE_COL).max(0.0));
    date -= take;
    excess -= take;
    let mut size = want_size;
    let take = excess.min((size - MIN_SIZE_COL).max(0.0));
    size -= take;

    // Even at their minimums the two columns do not fit, so they go: the name
    // is the one thing that has to stay readable. Size survives longest.
    if size + date > budget {
        return if budget >= MIN_SIZE_COL {
            (MIN_SIZE_COL, 0.0)
        } else {
            (0.0, 0.0)
        };
    }
    (size, date)
}

/// The widest a draggable column may become without squeezing the name.
///
/// The date column owns the space to its right, so its width and the other
/// column's width together decide how much room the name has left.
fn col_limit(layout: &RowLayout, other: f32) -> f32 {
    // name room = date_right - sp::MD - other - this_col - sp::MD - name_x
    let room = layout.date.x - sp::MD - other - sp::MD - layout.name.x;
    (room - MIN_NAME_COL).clamp(theme::col::MIN, theme::col::MAX)
}

fn click_kind(ui: &Ui) -> ClickKind {
    ui.input(|i| {
        if i.modifiers.ctrl || i.modifiers.command {
            ClickKind::Toggle
        } else if i.modifiers.shift {
            ClickKind::Range
        } else {
            ClickKind::Plain
        }
    })
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string())
}

/// A sensible starting folder.
fn default_dir() -> PathBuf {
    dirs::home_dir()
        .or_else(dirs::desktop_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn title_frame() -> Frame {
    Frame::new()
        .fill(c::PANEL)
        .inner_margin(Margin::ZERO)
        .outer_margin(Margin::ZERO)
}

fn toolbar_frame() -> Frame {
    Frame::new()
        .fill(c::PANEL)
        .inner_margin(Margin {
            left: sp::SM_I,
            right: sp::SM_I,
            top: 4,
            bottom: 0,
        })
        .outer_margin(Margin::ZERO)
}

/// The tab row sits on the same chrome as the toolbar, with no side padding of
/// its own because the tabs manage their own insets.
fn tab_frame() -> Frame {
    Frame::new()
        .fill(c::PANEL)
        .inner_margin(Margin::ZERO)
        .outer_margin(Margin::ZERO)
}

fn sidebar_frame() -> Frame {
    Frame::new()
        .fill(c::PANEL)
        .inner_margin(Margin::symmetric(sp::SM_I, sp::SM_I))
        .outer_margin(Margin::ZERO)
}

fn doc_frame() -> Frame {
    Frame::new()
        .fill(c::BG)
        .inner_margin(Margin::ZERO)
        .outer_margin(Margin::ZERO)
}

fn status_frame() -> Frame {
    Frame::new()
        .fill(c::PANEL)
        .inner_margin(Margin::ZERO)
        .outer_margin(Margin::ZERO)
}

// Where the tests say preferences live, for the thread they run on. Without it a test
// that builds the whole app reads the developer's own saved settings - a hidden
// preview, a remembered folder - and passes or fails depending on whose machine it
// is on.
#[cfg(test)]
thread_local! {
    static PREFS_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Persisted preferences live in a plain text file next to the log.
fn prefs_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = PREFS_OVERRIDE.with(|p| p.borrow().clone()) {
        return path;
    }
    dirs::config_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("rhumb")
        .join("prefs.txt")
}

/// Where a root's saved name index lives, beside the preferences. The name is a
/// stable hash of the path, so the same tree is found again next launch. Tests
/// use a folder of their own, so they never read or write the developer's cache.
pub(crate) fn index_cache_path(root: &Path) -> PathBuf {
    #[cfg(test)]
    let dir = std::env::temp_dir().join(format!("rhumb-index-cache-{}", std::process::id()));
    #[cfg(not(test))]
    let dir = dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("rhumb")
        .join("indexes");
    // `DefaultHasher` with its default keys is stable across runs, which is what
    // lets a file written before a restart be found after it.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut h);
    dir.join(format!("{:016x}.idx", h.finish()))
}

fn sort_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Size => "size",
        SortKey::Modified => "modified",
        SortKey::Ext => "ext",
    }
}

impl eframe::App for Rhumb {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.draw(ui, &ctx);
        if !self.shown {
            // The viewport is created hidden (`with_visible(false)`), so the
            // first frame is painted before the window appears. Showing it here
            // is what keeps the startup from flashing.
            self.shown = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // The window's own background, so the clear before the first frame is
        // never a flash of the default colour.
        c::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.write_prefs();
        // Whatever was brought out of an archive to be read goes with the window.
        archive::clear_own_cache();
        log::info!("rhumb exiting");
        // Prefs are written and the cache is gone; what is left is tearing down
        // the GPU device and joining workers, which is the wait after the window
        // has already vanished. The OS reclaims it faster.
        std::process::exit(0);
    }
}

// ---- helpers ---------------------------------------------------------------

/// Width of `text` in points, used for responsive layout maths.
fn text_width(ui: &Ui, text: &str) -> f32 {
    let font = egui::FontId::new(tfs::BODY, egui::FontFamily::Name(theme::FAMILY_UI.into()));
    ui.fonts_mut(|f| text.chars().map(|ch| f.glyph_width(&font, ch)).sum())
}

/// A small flat button for the toolbar's control cluster.
fn compact_button(ui: &mut Ui, label: &str, tip: &str) -> egui::Response {
    let galley = ui.painter().layout(
        label.to_owned(),
        theme::ui_font(tfs::SMALL),
        c::TEXT_DIM,
        f32::INFINITY,
    );
    let pad = Vec2::new(sp::SM, 3.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        let color = if resp.hovered() { c::TEXT } else { c::TEXT_DIM };
        let g = ui.painter().layout(
            label.to_owned(),
            theme::ui_font(tfs::SMALL),
            color,
            f32::INFINITY,
        );
        painter.galley(rect.min + pad, g, color);
    }
    resp.on_hover_text(tip)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod col_tests;

/// The whole app, driven a frame at a time the way a window would drive it.
///
/// The editor's own harness measures the editor; this measures what a person
/// actually waits for: the title bar, sidebar, file list, tabs and editor together,
/// with a real folder listing and a real file open. Typing into a document while
/// the file list behind it is drawn is the case the editor harness cannot see.
#[cfg(test)]
mod full_app;
