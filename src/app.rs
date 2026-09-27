//! The application: layout, navigation, selection, file operations and the
//! editor / live-preview pane.

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

use crate::editing;
use crate::editor::{self, Doc, DocKind};
use crate::fs_model::{self, Entry, SortKey};
use crate::markdown::Preview;
use crate::ops::{self, Clipboard};
use crate::search::{self, Search};
use crate::theme::{self, c, fs as tfs, sp};
use crate::thumbs::Thumbs;
use crate::tree::{self, Tree};
use crate::typeahead::{self, TypeAhead};
use crate::widgets::{self, Icon, RowCache, RowLayout, ViewMode};
use crate::workers::{self, Ids, Job, Msg, Outcome};

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
/// Sidebar width bounds.
const SIDEBAR_MIN: f32 = 150.0;
const SIDEBAR_MAX: f32 = 340.0;
/// Default and minimum width of the editor / preview pane.
const DOC_MIN: f32 = 380.0;
const DOC_DEFAULT: f32 = 640.0;

/// Most folders Quick access will hold, and so the most `pinN=` lines in prefs.
const MAX_PINS: usize = 24;
const ID_TITLE_DRAG: &str = "title_drag";
const ID_LIST: &str = "file_list";
const ID_COL_SIZE: &str = "col_size";
const ID_COL_DATE: &str = "col_date";
const ID_SEARCH: &str = "search_box";
const EDITOR_ID: &str = "xplor-editor";

/// State of the directory being shown.
enum Listing {
    Loading,
    Ready,
    Failed,
}

/// What a tab is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TabKind {
    /// An open document.
    File,
    /// The file list, opened as its own tab.
    Folder,
}

/// One open tab: either a document or the file list.
struct Tab {
    kind: TabKind,
    doc: Doc,
}

impl Tab {
    fn file(path: &Path) -> Tab {
        Tab {
            kind: TabKind::File,
            doc: Doc::placeholder(path),
        }
    }

    fn folder(path: &Path) -> Tab {
        Tab {
            kind: TabKind::Folder,
            doc: Doc::placeholder(path),
        }
    }

    fn path(&self) -> &Path {
        &self.doc.path
    }

    fn label(&self) -> String {
        match self.kind {
            TabKind::File => self.doc.file_name(),
            // A folder tab is named after the folder it was opened at.
            TabKind::Folder => self
                .doc
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Files".to_owned()),
        }
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

    /// True when the tab on screen is a document, which is what puts the
    /// editor up. A folder tab shows the file list instead.
    fn shows_file(&self) -> bool {
        self.active_tab().is_some_and(|t| t.kind == TabKind::File)
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

    /// Cycles to the next or previous tab, wrapping at both ends.
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

/// Modal dialogs.
enum Dialog {
    None,
    Rename {
        path: PathBuf,
        name: String,
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
    /// `close_app` distinguishes quitting from closing the file.
    Unsaved {
        path: PathBuf,
        close_app: bool,
    },
    Properties {
        path: PathBuf,
    },
    Help,
}

/// Back / forward history.
#[derive(Default)]
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
}

/// What a click in the list means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClickKind {
    Plain,
    Toggle,
    Range,
    Open,
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
    OpenEditor,
    CopyPath,
    OpenTerminal,
    OpenExternal,
    OpenNewWindow,
    Reveal,
    Copy,
    Cut,
    Rename,
    Delete,
    DeleteForever,
    Pin,
    Unpin,
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
    copy: bool,
    cut: bool,
    paste: bool,
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
    search_go: bool,
    escape: bool,
}

/// The application state.
pub struct Xplor {
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
    search_focus: bool,
    sidebar: bool,
    sidebar_w: f32,
    doc_w: f32,
    split: f32,
    row_cache: RowCache,

    // Recursive search
    search: Search,
    search_shown: bool,
    /// Whether typing searches below the current folder or only in it.
    scope: SearchScope,

    // Document
    /// The open tabs and which one is on screen.
    tabs: Tabs,
    loading: Option<PathBuf>,
    preview: Preview,
    render_version: u64,
    last_edit: Option<Instant>,
    wrap: bool,
    preview_visible: bool,

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
    /// Head of the text file the details pane is showing, with its path.
    peek: Option<(PathBuf, String)>,
    /// Path whose head is being read, so it is only requested once.
    peek_pending: Option<PathBuf>,
    /// Free space per volume, cached: reading it is milliseconds of work.
    free_space: fs_model::FreeSpace,
    /// Folder the pointer is over while dragging, if any.
    drop_target: Option<PathBuf>,
    /// Path whose context menu is up, if any. Held until egui reports the
    /// popup closed, so the menu survives past the frame of the right click.
    menu_path: Option<PathBuf>,
    /// Where that menu was opened. Captured on the click so the menu does not
    /// follow the pointer.
    menu_anchor: Option<Pos2>,
    /// Set on a right click and cleared the moment it is acted on, so opening
    /// the menu is a one-shot rather than something re-asserted every frame.
    menu_wanted: bool,
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
    /// Caret position in the open document, used by the editor shortcuts.
    editor_caret: Option<usize>,
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

impl Xplor {
    /// Builds the app and starts the first directory listing.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Xplor {
        let (tx, rx) = workers::bus();
        let arg = std::env::args().nth(1).map(PathBuf::from);

        let mut app = Xplor {
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
            search_focus: false,
            sidebar: true,
            sidebar_w: 196.0,
            doc_w: DOC_DEFAULT,
            split: 0.5,
            row_cache: RowCache::default(),
            search: Search::default(),
            search_shown: false,
            scope: SearchScope::Below,
            sidebar_tree: SidebarTree::default(),
            tabs: Tabs::default(),
            loading: None,
            preview: Preview::new(),
            render_version: 0,
            last_edit: None,
            wrap: false,
            preview_visible: true,
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
            peek: None,
            peek_pending: None,
            free_space: fs_model::FreeSpace::default(),
            drop_target: None,
            menu_path: None,
            menu_anchor: None,
            menu_wanted: false,
            pins: Pins::default(),
            measures: ops::Measures::new(tx.clone()),
            drag_payload: Vec::new(),
            search_typed: None,
            close_armed: false,
            editor_caret: None,
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
        app.request_listing();
        if let Some(p) = arg.filter(|p| p.is_file()) {
            app.open_path(&p);
        }
        app
    }

    // ---- persistence ---------------------------------------------------

    /// Reads `prefs.txt` from the user config directory. Missing or malformed
    /// entries simply fall back to the defaults, so a bad file can never stop
    /// the app from starting.
    ///
    /// `start_dir` is the folder named on the command line, if any. It wins
    /// over the remembered folder, so opening a path from a shell or a file
    /// manager lands where the user asked rather than where they were last.
    fn apply_prefs(&mut self, start_dir: Option<&Path>) {
        let Ok(text) = std::fs::read_to_string(prefs_path()) else {
            return;
        };
        let mut map: HashMap<&str, &str> = HashMap::new();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim(), v.trim());
            }
        }
        let b = |k: &str| map.get(k).and_then(|v| v.parse::<bool>().ok());
        let f = |k: &str| map.get(k).and_then(|v| v.parse::<f32>().ok());

        if let Some(v) = b("sidebar") {
            self.sidebar = v;
        }
        if let Some(v) = f("sidebar_w") {
            self.sidebar_w = v.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        }
        if let Some(v) = f("doc_w") {
            self.doc_w = v.max(DOC_MIN);
        }
        if let Some(v) = f("split") {
            self.split = v.clamp(0.2, 0.8);
        }
        if let Some(v) = b("ascending") {
            self.ascending = v;
        }
        if let Some(v) = b("show_hidden") {
            self.show_hidden = v;
        }
        if let Some(v) = b("wrap") {
            self.wrap = v;
        }
        if let Some(v) = b("preview_visible") {
            self.preview_visible = v;
        }
        if let Some(name) = map.get("sort") {
            self.sort = match *name {
                "size" => SortKey::Size,
                "modified" => SortKey::Modified,
                "ext" => SortKey::Ext,
                _ => SortKey::Name,
            };
        }
        if let Some(v) = map.get("col_size").and_then(|v| v.parse::<f32>().ok()) {
            self.col_size = v.clamp(theme::col::MIN, theme::col::MAX);
        }
        if let Some(v) = map.get("col_date").and_then(|v| v.parse::<f32>().ok()) {
            self.col_date = v.clamp(theme::col::MIN, theme::col::MAX);
        }
        if let Some(name) = map.get("view") {
            self.view = match *name {
                "List" => ViewMode::List,
                "Large icons" => ViewMode::Large,
                _ => ViewMode::Details,
            };
        }
        if let Some(v) = map.get("details") {
            self.details = *v != "0";
        }
        // Pinned folders are numbered so their order survives a rewrite.
        for i in 0..MAX_PINS {
            let key = format!("pin{i}");
            let Some(v) = map.get(key.as_str()) else {
                break;
            };
            if !v.is_empty() {
                let _ = self.pins.add(Path::new(v), MAX_PINS);
            }
        }
        match start_dir {
            Some(d) if d.is_dir() => self.cwd = fs_model::normalize(d),
            _ => {
                if let Some(cwd) = map.get("cwd")
                    && *cwd != "-"
                {
                    let p = PathBuf::from(cwd);
                    if p.is_dir() {
                        self.cwd = p;
                    }
                }
            }
        }
    }

    /// Writes preferences, plus the window size, to `prefs.txt`.
    fn write_prefs(&self) {
        let dir = prefs_path();
        let Some(parent) = dir.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let pins: String = self
            .pins
            .iter()
            .take(MAX_PINS)
            .enumerate()
            .map(|(i, p)| format!("pin{i}={}\n", p.to_string_lossy()))
            .collect();
        let body = format!(
            "sidebar={}\nsidebar_w={}\ndoc_w={}\nsplit={}\nsort={}\nascending={}\n\
             show_hidden={}\nwrap={}\ncol_size={}\ncol_date={}\npreview_visible={}\n\
             view={}\ndetails={}\ncwd={}\nwindow={}\n{pins}",
            self.sidebar,
            self.sidebar_w,
            self.doc_w,
            self.split,
            sort_name(self.sort),
            self.ascending,
            self.show_hidden,
            self.wrap,
            self.col_size,
            self.col_date,
            self.preview_visible,
            self.view.label(),
            self.details,
            self.cwd.to_string_lossy(),
            self.window_rect
                .map(|[x, y, w, h]| format!("{x},{y},{w},{h}"))
                .unwrap_or_else(|| "-".to_owned()),
        );
        if let Err(e) = std::fs::write(&dir, body) {
            log::warn!("cannot write prefs: {e}");
        }
    }

    // ---- quick access ----------------------------------------------------

    /// Adds a folder to Quick access, telling the user why if it cannot.
    ///
    /// Returns whether the list changed, so the caller can say so.
    fn pin(&mut self, path: &Path) -> bool {
        let result = self.pins.add(path, MAX_PINS);
        match result {
            Ok(()) => {
                self.write_prefs();
                true
            }
            Err(PinError::AlreadyThere) => {
                self.toast(String::from("Already in Quick access"));
                false
            }
            Err(PinError::Full) => {
                self.toast_err(format!("Quick access holds at most {MAX_PINS} folders"));
                false
            }
        }
    }

    /// Removes a folder from Quick access. Returns whether it was there.
    fn unpin(&mut self, path: &Path) -> bool {
        if !self.pins.remove(path) {
            return false;
        }
        self.write_prefs();
        true
    }

    fn is_pinned(&self, path: &Path) -> bool {
        self.pins.contains(path)
    }

    // ---- tabs -----------------------------------------------------------

    /// The tab on screen.
    fn tab(&self) -> Option<&Tab> {
        self.tabs.active_tab()
    }

    fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.active_tab_mut()
    }

    /// The document on screen, if any.
    fn doc(&self) -> Option<&Doc> {
        self.tab().map(|t| &t.doc)
    }

    fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.tab_mut().map(|t| &mut t.doc)
    }

    fn has_tabs(&self) -> bool {
        !self.tabs.is_empty()
    }

    fn shows_file_tab(&self) -> bool {
        self.tabs.shows_file()
    }

    /// Index of the tab showing `path`, if it is already open.
    fn tab_index(&self, path: &Path) -> Option<usize> {
        self.tabs.index_of(path)
    }

    /// Brings a tab to the front, rebuilding the preview cache if it changed.
    fn focus_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        // A folder tab shows the folder it was opened at.
        if self.tabs[index].kind == TabKind::Folder {
            let dir = self.tabs[index].doc.path.clone();
            self.tabs.focus(index);
            if dir.is_dir() && dir != self.cwd {
                self.navigate(&dir);
            }
            return;
        }
        if index == self.tabs.active {
            return;
        }
        self.tabs.focus(index);
        // The preview cache belongs to whichever document was on screen.
        self.preview.reset();
        self.render_version = 0;
        self.last_edit = None;
    }

    /// Cycles to the next or previous tab, the way Ctrl+Tab does.
    fn cycle_tab(&mut self, back: bool) {
        let before = self.tabs.active;
        self.tabs.cycle(back);
        if self.tabs.active != before {
            self.preview.reset();
            self.render_version = 0;
            self.last_edit = None;
        }
    }

    /// Closes one tab, moving focus to a neighbour.
    fn close_tab(&mut self, index: usize) {
        self.tabs.close(index);
        self.preview.reset();
        self.render_version = 0;
        self.last_edit = None;
    }

    // ---- frame ---------------------------------------------------------

    fn draw(&mut self, root: &mut Ui, ctx: &Context) {
        // Timed around the whole update, so the number is work done rather than
        // the gap between repaints (egui only repaints when something changes).
        let started = Instant::now();
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            self.window_rect = Some([rect.min.x, rect.min.y, rect.width(), rect.height()]);
        }

        self.drain_messages(ctx);
        self.pump_search();
        self.expire_toasts();
        self.handle_watch_debounce();
        self.close_guard(ctx);

        egui::containers::Panel::top("titlebar")
            .exact_size(sp::TITLE)
            .resizable(false)
            .frame(title_frame())
            .show(root, |ui| self.title_bar(ui));

        egui::containers::Panel::top("toolbar")
            .exact_size(sp::TOOLBAR)
            .resizable(false)
            .frame(toolbar_frame())
            .show(root, |ui| self.toolbar(ui));

        // The status bar claims the full window width, so it is shown before the
        // side panels: a panel only gets what the earlier ones left behind, and
        // Explorer's status bar runs edge to edge under everything.
        // The tab row spans the whole window, the way Explorer puts it above
        // the folder view and the document pane.
        if self.has_tabs() {
            egui::containers::Panel::top("tabs")
                .exact_size(sp::TAB_H)
                .resizable(false)
                .frame(tab_frame())
                .show(root, |ui| {
                    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
                    self.tab_strip(ui, rect);
                });
        }

        egui::containers::Panel::bottom("status")
            .exact_size(sp::STATUS)
            .resizable(false)
            .frame(status_frame())
            .show(root, |ui| self.status_ui(ui));

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

        // The right-hand pane is either the editor or, when nothing is open,
        // the details pane for the current selection. One pane, two jobs.
        if self.shows_file_tab() || (self.details && !self.sel.is_empty()) {
            let doc_w = self.doc_w;
            egui::containers::Panel::right("doc_panel")
                .resizable(true)
                .default_size(doc_w)
                .min_size(DOC_MIN)
                .show_separator_line(false)
                .frame(doc_frame())
                .show(root, |ui| {
                    let w = ui.max_rect().width();
                    if self.shows_file_tab() {
                        self.doc_ui(ui);
                    } else {
                        self.details_ui(ui);
                    }
                    if w > 0.0 {
                        self.doc_w = w;
                    }
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::central_panel(&theme::style()))
            .show(root, |ui| self.list_ui(ui));

        self.handle_file_drop(ctx);
        self.resize_edges(ctx);
        self.dialogs(ctx);
        self.draw_toasts(ctx);
        self.handle_keys(ctx);

        let done = started.elapsed().as_secs_f32() * 1000.0;
        self.perf.update_ms = if self.perf.update_ms == 0.0 {
            done
        } else {
            self.perf.update_ms * 0.9 + done * 0.1
        };
    }

    // ---- title bar -------------------------------------------------------

    /// The window is undecorated, so this bar owns moving, maximising and
    /// closing it, plus the resize edges along the bottom and right.
    fn title_bar(&mut self, ui: &mut Ui) {
        let height = sp::TITLE;
        let buttons_w = sp::WIN_BTN_W * 3.0;
        let rect = Rect::from_min_size(ui.min_rect().min, Vec2::new(ui.available_width(), height));

        // App mark.
        let mark = widgets::layout(ui, "xplor".to_owned(), theme::bold_font(11.5), c::TEXT_DIM);
        widgets::galley_at(
            ui.painter(),
            Pos2::new(rect.left() + sp::SM, rect.center().y - mark.size().y * 0.5),
            &mark,
            c::TEXT_DIM,
        );

        // Drag area: everything between the mark and the buttons.
        let drag = Rect::from_min_max(
            Pos2::new(rect.left() + 64.0, rect.top()),
            Pos2::new(rect.right() - buttons_w, rect.bottom()),
        );
        let resp = ui.interact(drag, Id::new(ID_TITLE_DRAG), Sense::click_and_drag());
        if resp.dragged() {
            // Re-sent every frame while held: the OS keeps moving the window
            // until the button comes up, which is how native drag works.
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        // Window buttons.
        let maximized = ui.input(|i| i.viewport().maximized).unwrap_or(false);
        let mut x = rect.right() - buttons_w;
        for (glyph, action) in [
            ("\u{2013}", WinAction::Minimize),
            (
                if maximized { "\u{2750}" } else { "\u{2610}" },
                WinAction::Maximize,
            ),
            ("\u{2715}", WinAction::Close),
        ] {
            let r = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(sp::WIN_BTN_W, height));
            let rresp = ui.interact(r, Id::new(("win", action as i32)), Sense::click());
            if ui.is_rect_visible(r) {
                let painter = ui.painter();
                if rresp.hovered() {
                    painter.rect_filled(r, CornerRadius::ZERO, c::HOVER);
                }
                let color = if rresp.hovered() {
                    c::TEXT
                } else {
                    c::TEXT_DIM
                };
                let g = widgets::layout(ui, glyph.to_owned(), theme::ui_font(11.0), color);
                let s = g.size();
                widgets::galley_at(
                    painter,
                    Pos2::new(r.center().x - s.x * 0.5, r.center().y - s.y * 0.5),
                    &g,
                    color,
                );
            }
            if rresp.clicked() {
                self.window_action(ui.ctx(), action);
            }
            x += sp::WIN_BTN_W;
        }

        // Hairline under the bar.
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.max.y - 0.5,
            Stroke::new(1.0, c::BORDER),
        );
    }

    /// Invisible drag strips along the window edges.
    ///
    /// An undecorated window keeps its native resize border on some platforms
    /// and loses it on others; these strips make resizing and edge snapping
    /// work everywhere, showing the right cursor while hovering.
    fn resize_edges(&mut self, ctx: &Context) {
        if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
            return;
        }
        let screen = ctx.input(|i| i.viewport_rect());
        let grab = 5.0f32;
        let corner = grab * 3.0;
        let (l, t, r, b) = (screen.left(), screen.top(), screen.right(), screen.bottom());
        let edges: [(Rect, egui::ResizeDirection, egui::CursorIcon); 8] = [
            (
                Rect::from_min_max(Pos2::new(l, t), Pos2::new(r, t + grab)),
                egui::ResizeDirection::North,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                Rect::from_min_max(Pos2::new(l, b - grab), Pos2::new(r, b)),
                egui::ResizeDirection::South,
                egui::CursorIcon::ResizeVertical,
            ),
            (
                Rect::from_min_max(Pos2::new(l, t), Pos2::new(l + grab, b)),
                egui::ResizeDirection::West,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                Rect::from_min_max(Pos2::new(r - grab, t), Pos2::new(r, b)),
                egui::ResizeDirection::East,
                egui::CursorIcon::ResizeHorizontal,
            ),
            (
                Rect::from_min_max(Pos2::new(l, t), Pos2::new(l + corner, t + corner)),
                egui::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNwSe,
            ),
            (
                Rect::from_min_max(Pos2::new(r - corner, t), Pos2::new(r, t + corner)),
                egui::ResizeDirection::NorthEast,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                Rect::from_min_max(Pos2::new(l, b - corner), Pos2::new(l + corner, b)),
                egui::ResizeDirection::SouthWest,
                egui::CursorIcon::ResizeNeSw,
            ),
            (
                Rect::from_min_max(Pos2::new(r - corner, b - corner), Pos2::new(r, b)),
                egui::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeNwSe,
            ),
        ];

        for (rect, dir, cursor) in edges {
            let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
                continue;
            };
            if !rect.contains(pos) {
                continue;
            }
            ctx.set_cursor_icon(cursor);
            if ctx.input_mut(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
            return;
        }
    }

    fn window_action(&mut self, ctx: &Context, action: WinAction) {
        match action {
            WinAction::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            WinAction::Maximize => {
                let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
            WinAction::Close => self.request_close(ctx),
        }
    }

    // ---- toolbar ---------------------------------------------------------

    /// One row, always: navigation, address bar, search, and the
    /// sort/filter controls. Widths are computed up front so nothing can
    /// overlap at any window size.
    fn toolbar(&mut self, ui: &mut Ui) {
        let total = ui.available_width();
        // Reserve room for the fixed clusters, then give the address bar what is
        // left. Below the floor, the address bar collapses to a Go button.
        let controls_w = 152.0f32; // New + sort + filter popups
        let search_w = (total * 0.24).clamp(110.0, 240.0);
        let nav_w = 4.0 * 26.0 + 3.0 * 2.0;
        let address_w = (total - nav_w - search_w - controls_w - sp::SM * 4.0).max(60.0);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);

            if widgets::icon_button(ui, Icon::Sidebar, "Toggle sidebar (Ctrl+B)").clicked() {
                self.sidebar = !self.sidebar;
            }
            if widgets::icon_button(ui, Icon::Back, "Back (Alt+Left)").clicked() {
                self.go_back();
            }
            if widgets::icon_button(ui, Icon::Forward, "Forward (Alt+Right)").clicked() {
                self.go_forward();
            }
            if widgets::icon_button(ui, Icon::Up, "Up (Alt+Up)").clicked() {
                self.go_up();
            }

            ui.add_space(sp::SM);
            self.address_bar(ui, address_w);
            ui.add_space(sp::SM);

            self.search_box(ui, search_w);
            ui.add_space(sp::SM);
            self.new_button(ui);
            self.sort_button(ui);
            self.filter_button(ui);
        });
        // Hairline under the toolbar.
        let rect = ui.max_rect().expand2(Vec2::new(0.0, 1.0));
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.max.y,
            Stroke::new(1.0, c::BORDER),
        );
    }

    /// The "New" menu: folder, text document, or a compressed copy of the
    /// selection, which is what Explorer's own menu offers.
    fn new_button(&mut self, ui: &mut Ui) {
        let resp = widgets::flat_button(ui, "New \u{25BE}", "Create something (Alt+F)");
        // Alt+F asks for the menu, so it is opened by id rather than by click.
        let id = resp.id.with("menu");
        if std::mem::take(&mut self.new_menu) {
            egui::Popup::open_id(ui.ctx(), id);
        }
        let mut folder = false;
        let mut document = false;
        let mut from_selection = false;
        egui::Popup::menu(&resp).id(id).show(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(sp::SM, 2.0);
            ui.set_min_width(190.0);
            if ui.button("Folder").clicked() {
                folder = true;
            }
            if ui.button("Text Document").clicked() {
                document = true;
            }
            let selectable = !self.sel.is_empty();
            ui.add_enabled(selectable, egui::Button::new("Compressed (zipped) folder"))
                .on_disabled_hover_text("Select something first")
                .clicked()
                .then(|| from_selection = true);
        });
        if folder {
            self.dialog = Dialog::Create {
                dir: self.cwd.clone(),
                name: String::new(),
                folder: true,
            };
        }
        if document {
            self.dialog = Dialog::Create {
                dir: self.cwd.clone(),
                name: String::new(),
                folder: false,
            };
        }
        if from_selection {
            self.start_zip();
        }
    }

    /// The address bar: clickable path segments that elide instead of
    /// overflowing, with the deepest segments kept visible.
    fn address_bar(&mut self, ui: &mut Ui, width: f32) {
        let mut parts = fs_model::breadcrumbs(&self.cwd);
        let last = parts.len().saturating_sub(1);
        let mut target: Option<PathBuf> = None;

        // Drop leading segments until the rest fits, keeping a way back.
        loop {
            let needed: f32 = parts
                .iter()
                .enumerate()
                .map(|(i, (label, _))| {
                    let w = text_width(ui, label) + sp::XS * 2.0 + 3.0;
                    if i > 0 { w + 12.0 } else { w }
                })
                .sum();
            if needed <= width || parts.len() <= 2 {
                break;
            }
            parts.remove(0);
        }

        if parts.first().is_none_or(|(label, _)| label.is_empty()) && parts.len() == 1 {
            // A bare root, e.g. a drive: render it plainly.
        }

        for (i, (label, path)) in parts.iter().enumerate() {
            if i > 0 {
                widgets::breadcrumb_separator(ui);
            }
            let remaining = (width - ui.min_rect().left() + ui.max_rect().left()).max(40.0);
            let resp = widgets::breadcrumb_segment(ui, label, i == last, remaining);
            if resp.clicked() {
                target = Some(path.clone());
            }
        }
        if let Some(p) = target {
            self.navigate(&p);
        }
    }

    /// Sort column and direction.
    fn sort_button(&mut self, ui: &mut Ui) {
        let mut action = None;
        let resp = compact_button(
            ui,
            &format!(
                "{} {}",
                self.sort.label(),
                if self.ascending {
                    "\u{2191}"
                } else {
                    "\u{2193}"
                }
            ),
            "Sort order",
        );
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_width(150.0);
            for key in [
                SortKey::Name,
                SortKey::Size,
                SortKey::Modified,
                SortKey::Ext,
            ] {
                let active = self.sort == key;
                if ui.selectable_label(active, key.label()).clicked() {
                    action = Some(key);
                }
            }
            ui.separator();
            if ui.selectable_label(self.ascending, "Ascending").clicked() {
                action = Some(self.sort);
                self.ascending = true;
            }
            if ui.selectable_label(!self.ascending, "Descending").clicked() {
                action = Some(self.sort);
                self.ascending = false;
            }
        });
        if let Some(key) = action {
            if self.sort == key {
                self.ascending = !self.ascending;
            } else {
                self.sort = key;
                self.ascending = true;
            }
            self.apply_sort();
        }
    }

    /// View options: hidden files and search scope.
    fn filter_button(&mut self, ui: &mut Ui) {
        let mut hidden = None;
        let mut scope = None;
        let glyph = if self.show_hidden {
            "\u{25CF} Filter"
        } else {
            "\u{25CB} Filter"
        };
        let resp = compact_button(ui, glyph, "View options");
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_width(190.0);
            if ui
                .checkbox(&mut self.show_hidden, "Show hidden files (Ctrl+H)")
                .changed()
            {
                hidden = Some(self.show_hidden);
            }
            ui.separator();
            if ui
                .selectable_label(self.scope == SearchScope::Below, "Search below this folder")
                .clicked()
            {
                scope = Some(SearchScope::Below);
            }
            if ui
                .selectable_label(self.scope == SearchScope::Here, "This folder only")
                .clicked()
            {
                scope = Some(SearchScope::Here);
            }
        });
        if hidden.is_some() {
            self.request_listing();
        }
        if let Some(s) = scope {
            self.scope = s;
            self.on_filter_changed();
        }
    }

    fn search_box(&mut self, ui: &mut Ui, width: f32) {
        let mut text = self.filter.clone();
        let running = self.search.running;
        let searched = self.search_shown && !self.search.results.is_empty();

        let out = Frame::new()
            .fill(c::CODE_BG)
            .stroke(Stroke::new(1.0, c::BORDER))
            .corner_radius(CornerRadius::same(sp::RADIUS))
            .inner_margin(Margin::symmetric(sp::SM_I, 3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(sp::XS, 0.0);
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                if ui.is_rect_visible(icon_rect) {
                    Icon::Search.paint(
                        ui.painter(),
                        icon_rect,
                        if running { c::TEXT } else { c::TEXT_GHOST },
                    );
                }
                let hint = match (searched, self.scope) {
                    (true, _) => format!("{} matches", self.search.results.len()),
                    (false, SearchScope::Below) => "Search below".to_owned(),
                    (false, SearchScope::Here) => "Filter here".to_owned(),
                };
                TextEdit::singleline(&mut text)
                    .id(Id::new(ID_SEARCH))
                    .hint_text(hint)
                    .frame(Frame::NONE)
                    .desired_width((width - 34.0).max(40.0))
                    .text_color(c::TEXT)
                    .show(ui)
                    .response
            })
            .response;

        if self.search_focus {
            out.request_focus();
            self.search_focus = false;
        }
        if out.changed() {
            self.filter = text;
            self.on_filter_changed();
        }
    }

    // ---- sidebar ----------------------------------------------------------

    // ---- sidebar ----------------------------------------------------------

    /// Explorer-style tree: quick access and drives, each expandable to reveal
    /// subfolders. Children are read lazily on a worker thread, so expanding
    /// never blocks the UI.
    fn sidebar_ui(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let mut nav: Option<PathBuf> = None;
        let mut toggle: Option<PathBuf> = None;
        let mut expand: Option<PathBuf> = None;

        // Quick access sits above everything else, the way Explorer puts it.
        // The section only appears once something is pinned, so an unused
        // sidebar stays quiet.
        if !self.pins.is_empty() {
            self.quick_access_ui(ui, &mut nav);
        }

        // Roots: quick access, then devices. Labels stay friendly; the tree
        // itself only cares about paths.
        let mut roots: Vec<(String, PathBuf, bool)> = Vec::new();
        for p in fs_model::places() {
            let device = p.is_device();
            roots.push((p.label, p.path, device));
        }
        for d in fs_model::drives() {
            roots.push((d.label, d.path, true));
        }
        let paths: Vec<PathBuf> = roots.iter().map(|(_, p, _)| p.clone()).collect();
        self.sidebar_tree.tree.set_roots(&paths);

        // The tree already holds the rows in the right order, roots and
        // descendants interleaved. Walk it in that order and only swap in the
        // friendly label and device flag for the root rows - appending the
        // descendants afterwards would pile every folder's children up at the
        // bottom of the list instead of under their parent.
        let friendly: std::collections::HashMap<&Path, (&str, bool)> = roots
            .iter()
            .map(|(label, path, device)| (path.as_path(), (label.as_str(), *device)))
            .collect();
        let mut lines: Vec<(String, PathBuf, usize, bool, bool)> = Vec::new();
        for row in self.sidebar_tree.tree.rows() {
            if let Some((label, device)) = friendly.get(row.path.as_path()) {
                lines.push((
                    (*label).to_owned(),
                    row.path.clone(),
                    0,
                    row.expanded,
                    *device,
                ));
            } else {
                lines.push((
                    row.label.clone(),
                    row.path.clone(),
                    row.depth,
                    row.expanded,
                    false,
                ));
            }
        }

        for (label, path, depth, expanded, is_device) in lines {
            if self.tree_row(
                ui,
                &label,
                &path,
                depth,
                expanded,
                is_device,
                &mut nav,
                &mut toggle,
                &mut expand,
            ) {
                continue;
            }
        }

        let rect = ui.max_rect();
        ui.painter().vline(
            rect.right(),
            rect.top()..=rect.bottom(),
            Stroke::new(1.0, c::BORDER),
        );

        if let Some(p) = expand {
            self.load_tree_children(&p);
        }
        if let Some(p) = toggle {
            self.expand_tree(&p);
        }
        if let Some(p) = nav
            && p != self.cwd
        {
            self.navigate(&p);
        }
    }

    /// The pinned folders at the top of the sidebar.
    ///
    /// Drawn separately from the tree because pinning is about one folder, not
    /// a place in a hierarchy: no chevron, no children, no drop target.
    fn quick_access_ui(&mut self, ui: &mut Ui, nav: &mut Option<PathBuf>) {
        // Header: a pin glyph and a label, in the same quiet style the
        // properties sheet uses for its section titles.
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), sp::SECTION), Sense::hover());
        let painter = ui.painter();
        let icon = Rect::from_center_size(
            Pos2::new(rect.left() + 8.0, rect.center().y),
            Vec2::splat(11.0),
        );
        Icon::Pin.paint(painter, icon, c::TEXT_GHOST);
        let g = widgets::layout(
            ui,
            String::from("Quick access"),
            theme::bold_font(10.0),
            c::TEXT_GHOST,
        );
        widgets::galley_at(
            painter,
            Pos2::new(
                icon.right() + sp::XS + 1.0,
                rect.center().y - g.size().y * 0.5,
            ),
            &g,
            c::TEXT_GHOST,
        );

        for path in self.pins.clone() {
            let label = path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            if self.pin_row(ui, &label, &path, nav) {
                // Same menu as everywhere else, so it offers to unpin.
                self.open_context_menu(ui, &path);
            }
        }
        ui.add_space(sp::XS);
    }

    /// One pinned row. Returns whether the row asked to be unpinned, via a
    /// right click.
    fn pin_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        path: &Path,
        nav: &mut Option<PathBuf>,
    ) -> bool {
        let active = self.cwd == path;
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(width, sp::NAV_ROW),
            Sense::click().union(Sense::drag()),
        );
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if active {
                painter.rect_filled(rect, CornerRadius::same(4), c::SEL);
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(rect.left() + 1.0, rect.top() + 4.0),
                        Vec2::new(2.0, sp::NAV_ROW - 8.0),
                    ),
                    1.0,
                    c::ACCENT,
                );
            } else if resp.hovered() {
                painter.rect_filled(rect, CornerRadius::same(4), c::HOVER);
            }
            // A pin, not a chevron: there is nothing here to expand.
            let icon = Rect::from_center_size(
                Pos2::new(rect.left() + 16.0, rect.center().y),
                Vec2::splat(13.0),
            );
            let icon_color = if active { c::SEL_TEXT } else { c::TEXT_FAINT };
            Icon::Pin.paint(painter, icon, icon_color);
            Icon::Folder.paint(
                painter,
                Rect::from_center_size(
                    Pos2::new(icon.right() + 9.0, rect.center().y),
                    Vec2::splat(13.0),
                ),
                icon_color,
            );
            let text_x = icon.right() + 9.0 + 13.0 + sp::XS + 2.0;
            let text_color = if active { c::SEL_TEXT } else { c::TEXT_DIM };
            let g = widgets::layout_elided(
                ui,
                label.to_owned(),
                theme::ui_font(tfs::BODY),
                text_color,
                (rect.right() - text_x - sp::SM).max(20.0),
            );
            widgets::galley_at(
                painter,
                Pos2::new(text_x, rect.center().y - g.size().y * 0.5),
                &g,
                text_color,
            );
        }
        if resp.clicked() {
            *nav = Some(path.to_path_buf());
        }
        resp.secondary_clicked()
    }

    /// One sidebar line: chevron, icon, label, optional free-space note.
    ///
    /// Actions are collected into the out-parameters so the caller can apply
    /// them once layout is done.
    #[allow(clippy::too_many_arguments)]
    fn tree_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        path: &Path,
        depth: usize,
        expanded: bool,
        is_device: bool,
        nav: &mut Option<PathBuf>,
        toggle: &mut Option<PathBuf>,
        expand: &mut Option<PathBuf>,
    ) -> bool {
        let active = self.cwd == path;
        let loading = self.sidebar_tree.tree.is_pending(path);
        let width = ui.available_width();
        let indent = depth as f32 * sp::INDENT;
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(width, sp::NAV_ROW),
            Sense::click().union(Sense::drag()),
        );
        if resp.hovered() && !is_device {
            self.drop_target = Some(path.to_path_buf());
        }
        let chevron_x = rect.left() + 6.0 + indent;

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if active {
                painter.rect_filled(rect, CornerRadius::same(4), c::SEL);
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(rect.left() + 1.0, rect.top() + 4.0),
                        Vec2::new(2.0, sp::NAV_ROW - 8.0),
                    ),
                    1.0,
                    c::ACCENT,
                );
            } else if resp.hovered() {
                painter.rect_filled(rect, CornerRadius::same(4), c::HOVER);
            }

            // Chevron, then the folder or drive glyph.
            let chevron = Rect::from_center_size(
                Pos2::new(chevron_x + 5.0, rect.center().y),
                Vec2::splat(12.0),
            );
            if loading {
                Icon::Sort.paint(painter, chevron, c::TEXT_GHOST);
            } else if !is_device {
                Icon::Chevron.paint_with(
                    painter,
                    chevron,
                    if expanded { c::TEXT_DIM } else { c::TEXT_GHOST },
                    expanded,
                );
            }

            let icon = Rect::from_center_size(
                Pos2::new(chevron.max.x + 9.0, rect.center().y),
                Vec2::splat(13.0),
            );
            let icon_color = if active { c::SEL_TEXT } else { c::TEXT_FAINT };
            if is_device {
                Icon::Sidebar.paint(painter, icon, icon_color);
            } else {
                Icon::Folder.paint(painter, icon, icon_color);
            }

            let text_x = icon.right() + sp::XS + 2.0;
            let text_color = if active { c::SEL_TEXT } else { c::TEXT_DIM };
            let note = if is_device && !active {
                self.free_space
                    .get(path)
                    .map(|(avail, _)| format!("{} free", fs_model::fmt_size(avail)))
            } else {
                None
            };
            let note_w = if note.is_some() { 74.0 } else { 0.0 };
            let g = widgets::layout_elided(
                ui,
                label.to_owned(),
                theme::ui_font(tfs::BODY),
                text_color,
                (rect.right() - text_x - note_w - sp::SM).max(20.0),
            );
            widgets::galley_at(
                painter,
                Pos2::new(text_x, rect.center().y - g.size().y * 0.5),
                &g,
                text_color,
            );
            if let Some(note) = note {
                let mg = widgets::layout_elided(
                    ui,
                    note,
                    theme::ui_font(tfs::SMALL),
                    c::TEXT_GHOST,
                    note_w,
                );
                widgets::text_right(
                    painter,
                    Pos2::new(rect.right() - sp::SM, rect.center().y),
                    &mg,
                    c::TEXT_GHOST,
                );
            }
        }

        // The leading chevron area expands; the rest of the row navigates.
        if resp.clicked() {
            let on_chevron = ui.input(|i| {
                i.pointer
                    .latest_pos()
                    .is_some_and(|p| p.x < chevron_x + 14.0)
            });
            if on_chevron {
                if loading {
                    *expand = Some(path.to_path_buf());
                } else {
                    *toggle = Some(path.to_path_buf());
                }
            } else {
                *nav = Some(path.to_path_buf());
            }
        }
        false
    }

    /// Asks for a folder's subfolders, read on a worker thread.
    fn load_tree_children(&mut self, path: &Path) {
        self.sidebar_tree.loading = Some(path.to_path_buf());
        self.sidebar_tree.tree.set_pending(path.to_path_buf());
        let tx = self.tx.clone();
        let path = path.to_path_buf();
        let _ = std::thread::Builder::new()
            .name("xplor-tree".into())
            .spawn(move || {
                let dirs = tree::read_dirs(&path);
                let _ = tx.send(Msg::TreeLoaded { path, dirs });
            });
    }

    /// Expands or collapses a folder, loading children the first time.
    fn expand_tree(&mut self, path: &Path) {
        if self.sidebar_tree.tree.is_expanded(path) {
            self.sidebar_tree.tree.collapse_root(path);
            return;
        }
        let needs = self.sidebar_tree.tree.expand_root(path);
        if let Some(p) = needs {
            self.load_tree_children(&p);
        }
    }

    // ---- the file list ----------------------------------------------------

    /// The file list. One code path serves all three views: only the cell
    /// geometry and the painter change.
    fn list_ui(&mut self, ui: &mut Ui) {
        let started = Instant::now();
        let searching = self.searching();
        let count = self.row_count();
        let width = ui.available_width();
        let view = self.view;

        // Details view has sortable column headers; the other two do not.
        let mut layout = RowLayout::default();
        if view.has_columns() {
            let header_h = 26.0f32;
            // Allocated, so the rows below start underneath it.
            let (header, _) = ui.allocate_exact_size(Vec2::new(width, header_h), Sense::hover());
            let (fit_size, fit_date) = fit_columns(width, self.col_size, self.col_date);
            layout = RowLayout::new(header, fit_size, fit_date);
            widgets::list_header(ui, header, self.sort, self.ascending, fit_size, fit_date);

            let mut header_click = None;
            // A dropped column has no header to click either.
            let mut targets: Vec<(SortKey, f32, bool)> =
                vec![(SortKey::Name, layout.name.x, false)];
            if layout.shows_size() {
                targets.push((SortKey::Size, layout.size_left() + 8.0, true));
            }
            if layout.shows_date() {
                targets.push((SortKey::Modified, layout.date_left() + 8.0, true));
            }
            for (key, x, right) in targets {
                let r = if right {
                    Rect::from_min_max(
                        Pos2::new(x, header.top()),
                        Pos2::new(header.right() - sp::SM, header.bottom()),
                    )
                } else {
                    Rect::from_min_max(
                        Pos2::new(header.left(), header.top()),
                        Pos2::new(layout.size_left(), header.bottom()),
                    )
                };
                if ui
                    .interact(r, Id::new(("sort", key.label())), Sense::click())
                    .clicked()
                {
                    header_click = Some(key);
                }
            }

            // Drag the dividers to resize, the way Explorer does. The grab area
            // is wider than the line it draws, so it is easy to catch. A column
            // that was dropped for want of room has no divider to grab.
            let mut dividers: Vec<(&str, f32)> = Vec::with_capacity(2);
            if layout.shows_size() {
                dividers.push((ID_COL_SIZE, layout.size_left()));
            }
            if layout.shows_date() {
                dividers.push((ID_COL_DATE, layout.date_left()));
            }
            for (id, x) in dividers {
                let grab = Rect::from_center_size(
                    Pos2::new(x, header.center().y),
                    Vec2::new(theme::col::GRAB * 2.0, header.height()),
                );
                let resp = ui.interact(grab, Id::new(id), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    ui.painter().vline(
                        x,
                        header.y_range(),
                        Stroke::new(
                            1.0,
                            if resp.dragged() {
                                c::ACCENT
                            } else {
                                c::TEXT_GHOST
                            },
                        ),
                    );
                }
                if resp.dragged() {
                    let delta = resp.drag_delta().x;
                    // Dragging left widens the column, so the delta is negated.
                    let limit = |other: f32| col_limit(&layout, other);
                    if id == ID_COL_SIZE {
                        self.col_size =
                            (self.col_size - delta).clamp(theme::col::MIN, limit(self.col_date));
                    } else {
                        self.col_date =
                            (self.col_date - delta).clamp(theme::col::MIN, limit(self.col_size));
                    }
                    // The name column and every row's shaped text change width.
                    self.row_cache.clear();
                    let (s, d) = fit_columns(width, self.col_size, self.col_date);
                    layout = RowLayout::new(header, s, d);
                }
            }

            if let Some(key) = header_click {
                if self.sort == key {
                    self.ascending = !self.ascending;
                } else {
                    self.sort = key;
                    self.ascending = true;
                }
                self.apply_sort();
            }
        } else {
            // A hairline instead of a header, so the list still has a top edge.
            ui.allocate_exact_size(Vec2::new(width, 1.0), Sense::hover());
            ui.painter().hline(
                ui.min_rect().left()..=ui.max_rect().right(),
                ui.min_rect().top(),
                egui::Stroke::new(1.0, c::DIVIDER),
            );
        }

        if count == 0 {
            ui.allocate_exact_size(Vec2::new(width, 0.0), Sense::hover());
            let (title, sub) = if searching {
                if self.search.running {
                    ("Searching\u{2026}", "Scanning subfolders")
                } else {
                    (
                        "No matches",
                        "Press Enter in the box to search here and below",
                    )
                }
            } else if !self.filter.is_empty() {
                ("No matches", "Clear the filter to see everything")
            } else if matches!(self.listing, Listing::Failed) {
                (
                    "Folder unavailable",
                    "It may have been moved, or you may not have access",
                )
            } else {
                ("Empty folder", "Nothing to show")
            };
            widgets::empty_state(ui, title, sub);
            self.perf.list_ms = started.elapsed().as_secs_f32() * 1000.0;
            return;
        }

        // Cell geometry: full-width rows, or a grid of tiles.
        let grid = view.is_grid();
        let cell_h = if grid { sp::TILE } else { sp::ROW };
        let cols = if grid {
            ((width - sp::SM) / sp::TILE_W).floor().max(1.0) as usize
        } else {
            1
        };
        let rows_total = count.div_ceil(cols);
        let name_w = if grid {
            sp::TILE_W - sp::SM * 2.0
        } else if view.has_columns() {
            layout.name_limit() - layout.name.x
        } else {
            (width - 74.0).max(40.0)
        };

        let mut click: Option<(usize, ClickKind)> = None;
        // Collected inside the loop and applied after it: the closure borrows
        // list fields, so it cannot reach into the app to open a menu itself.
        let mut context: Option<PathBuf> = None;
        let scroll_to = self.scroll_to.take();

        {
            // Immutable list data for the paint loop; the caches are separate
            // fields, so the borrow checker lets us touch them here.
            let entries = &self.entries;
            let visible = &self.visible;
            let results = &self.search.results;
            let sel = &self.sel;

            let mut area = egui::ScrollArea::vertical()
                .id_salt(ID_LIST)
                .auto_shrink([false, false]);
            if let Some((row, _align)) = scroll_to {
                area = area.vertical_scroll_offset(row as f32 * cell_h);
            }
            area.show(ui, |ui| {
                let (content, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width().max(1.0), rows_total as f32 * cell_h),
                    Sense::hover(),
                );
                let clip = ui.clip_rect();
                let scrolled = clip.min.y - content.min.y;
                // Only the rows that intersect the clip are ever built.
                let first_row = ((scrolled / cell_h).floor().max(0.0) as usize).min(rows_total);
                let visible_rows = (clip.height() / cell_h).ceil() as usize + 2;
                let last_row = (first_row + visible_rows).min(rows_total);

                for r in first_row..last_row {
                    for c in 0..cols {
                        let i = r * cols + c;
                        if i >= count {
                            break;
                        }
                        let entry: &Entry = if searching {
                            match results.get(i) {
                                Some(e) => e,
                                None => continue,
                            }
                        } else {
                            match visible.get(i).and_then(|e| entries.get(*e)) {
                                Some(e) => e,
                                None => continue,
                            }
                        };
                        let cell = if grid {
                            Rect::from_min_size(
                                Pos2::new(
                                    content.min.x + c as f32 * sp::TILE_W,
                                    content.min.y + r as f32 * cell_h,
                                ),
                                Vec2::new(sp::TILE_W, cell_h),
                            )
                        } else {
                            Rect::from_min_size(
                                Pos2::new(content.min.x, content.min.y + i as f32 * cell_h),
                                Vec2::new(content.width(), cell_h),
                            )
                        };
                        let resp = ui.interact(
                            cell,
                            Id::new(("row", i)),
                            Sense::click().union(Sense::drag()),
                        );
                        // A folder under the pointer is a drop target.
                        if entry.is_dir && resp.hovered() {
                            self.drop_target = Some(entry.path.clone());
                        }
                        if grid {
                            let name = self.row_cache.tile_name(ui, i, entry, name_w);
                            let thumb = if entry.is_dir {
                                None
                            } else {
                                self.thumbs.get(&entry.path, sp::THUMB_PX)
                            };
                            widgets::paint_tile(
                                ui,
                                entry,
                                cell,
                                sel.contains(&entry.path),
                                resp.hovered(),
                                &name,
                                thumb.as_ref(),
                            );
                        } else {
                            let galleys: &widgets::RowGalleys =
                                self.row_cache.get_or_build(ui, i, entry, name_w);
                            widgets::paint_row(
                                ui,
                                entry,
                                &layout,
                                cell,
                                sel.contains(&entry.path),
                                resp.hovered(),
                                galleys,
                                view.has_columns(),
                            );
                        }
                        if resp.clicked() {
                            click = Some((i, click_kind(ui)));
                        }
                        if resp.double_clicked() {
                            click = Some((i, ClickKind::Open));
                        }
                        if resp.secondary_clicked() {
                            context = Some(entry.path.clone());
                            click = Some((i, ClickKind::Plain));
                        }
                    }
                }
            });
        }

        if let Some((i, kind)) = click {
            self.handle_click(i, kind);
        }
        if let Some(path) = context {
            self.open_context_menu(ui, &path);
        }
        // Dragging out of a selected row starts an in-app file drag; releasing
        // over a folder row moves or copies the selection there.
        let moved_far = ui.input(|i| {
            let Some(origin) = i.pointer.press_origin() else {
                return false;
            };
            let Some(now) = i.pointer.hover_pos() else {
                return false;
            };
            (now - origin).length() > 5.0 && i.pointer.button_down(egui::PointerButton::Primary)
        });
        if self.drag_payload.is_empty() && moved_far && !self.sel.is_empty() {
            self.drag_payload = self.sel.iter().cloned().collect();
        } else if !self.drag_payload.is_empty() && !ui.input(|i| i.pointer.any_down()) {
            self.drag_payload.clear();
        }
        // `menu_path` outlives the click that opened it: a popup is only drawn
        // on the frames its function runs, so stop asking and it vanishes.
        if let Some(path) = self.menu_path.clone() {
            self.context_menu(ui, &path);
        }
        self.perf.list_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    fn handle_click(&mut self, i: usize, kind: ClickKind) {
        let Some(path) = self.path_at(i) else { return };
        match kind {
            ClickKind::Plain => {
                self.sel.clear();
                self.sel.insert(path);
                self.cursor = i;
                self.anchor = i;
            }
            ClickKind::Toggle => {
                if !self.sel.remove(&path) {
                    self.sel.insert(path);
                }
                self.cursor = i;
                self.anchor = i;
            }
            ClickKind::Range => {
                let lo = self.anchor.min(i);
                let hi = self.anchor.max(i);
                self.sel.clear();
                for k in lo..=hi {
                    if let Some(p) = self.path_at(k) {
                        self.sel.insert(p);
                    }
                }
                self.cursor = i;
            }
            ClickKind::Open => self.open_path(&path),
        }
    }

    /// Records a right click so the menu opens on this frame.
    ///
    /// Both the click position and the path are captured here. The position has
    /// to be remembered rather than read per frame, or the menu would slide
    /// along behind the pointer as it moves.
    fn open_context_menu(&mut self, ui: &Ui, path: &Path) {
        self.menu_path = Some(path.to_path_buf());
        self.menu_anchor = ui.input(|i| i.pointer.interact_pos());
        self.menu_wanted = true;
    }

    /// Right-click menu.
    ///
    /// The popup is anchored to the pointer, not to the `Ui` cursor. The list is
    /// virtualized, so by the time we get here the cursor can be millions of
    /// pixels below the viewport, and a menu anchored there opens off-screen.
    fn context_menu(&mut self, ui: &mut Ui, path: &Path) {
        let path = path.to_path_buf();
        let is_dir = path.is_dir();
        let editable = !is_dir && fs_model::is_editable_text(&path);
        let pinned = self.is_pinned(&path);
        let mut action: Option<CtxAction> = None;

        // The anchor is the spot that was clicked, recorded when the right
        // click landed. Reading the pointer every frame instead would drag the
        // menu along behind the mouse.
        let anchor = self.menu_anchor.unwrap_or_else(|| {
            ui.input(|i| i.pointer.interact_pos())
                .unwrap_or_else(|| ui.max_rect().center())
        });
        let id = Id::new(("ctx_menu", &path));
        let dummy = ui.interact(Rect::from_min_size(anchor, Vec2::ZERO), id, Sense::click());

        let popup = egui::Popup::menu(&dummy).id(id);
        let was_open = popup.is_open();
        // Open exactly once, on the frame the right click landed. After that
        // egui owns the state: it closes on a click anywhere or on Escape, and
        // we must not re-open behind its back or the menu could never be
        // dismissed.
        let open = std::mem::take(&mut self.menu_wanted)
            .then_some(egui::containers::SetOpenCommand::Bool(true));

        let shown = popup.open_memory(open).width(200.0).show(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(sp::SM, 3.0);
            if editable && ui.button("Open in editor").clicked() {
                action = Some(CtxAction::OpenEditor);
            }
            if ui.button("Open with system app").clicked() {
                action = Some(CtxAction::OpenExternal);
            }
            if is_dir && ui.button("Open in new window").clicked() {
                action = Some(CtxAction::OpenNewWindow);
            }
            if ui.button("Show in file manager").clicked() {
                action = Some(CtxAction::Reveal);
            }
            if ui.button("Open in Terminal").clicked() {
                action = Some(CtxAction::OpenTerminal);
            }
            ui.add_space(sp::XS);
            ui.separator();
            ui.add_space(sp::XS);
            if ui.button("Copy").clicked() {
                action = Some(CtxAction::Copy);
            }
            if ui.button("Cut").clicked() {
                action = Some(CtxAction::Cut);
            }
            if ui.button("Copy as path").clicked() {
                action = Some(CtxAction::CopyPath);
            }
            if is_dir {
                if pinned {
                    if ui.button("Unpin from Quick access").clicked() {
                        action = Some(CtxAction::Unpin);
                    }
                } else if ui.button("Pin to Quick access").clicked() {
                    action = Some(CtxAction::Pin);
                }
            }
            if ui.button("Rename\u{2026}").clicked() {
                action = Some(CtxAction::Rename);
            }
            ui.add_space(sp::XS);
            ui.separator();
            ui.add_space(sp::XS);
            if ui.button("Move to trash").clicked() {
                action = Some(CtxAction::Delete);
            }
            if ui.button("Delete permanently\u{2026}").clicked() {
                action = Some(CtxAction::DeleteForever);
            }
        });

        // egui closed the menu: a click anywhere, or Escape. Drop the path so
        // the caller stops asking for a menu that is no longer up. A popup is
        // only drawn on the frames its function runs, so one missed frame
        // would make it vanish anyway.
        if was_open && shown.is_none() {
            self.menu_path = None;
            self.menu_anchor = None;
        }

        let Some(action) = action else { return };
        match action {
            CtxAction::OpenEditor => self.open_path(&path),
            CtxAction::OpenExternal => {
                if let Err(e) = editor::open_externally(&path) {
                    self.toast_err(e);
                }
            }
            CtxAction::OpenNewWindow => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg(&path).spawn();
                }
            }
            CtxAction::Reveal => editor::reveal_in_file_manager(&path),
            CtxAction::Copy => self.copy_selection(false),
            CtxAction::CopyPath => self.copy_as_path(&path),
            CtxAction::OpenTerminal => self.open_in_terminal(&path),
            CtxAction::Cut => self.copy_selection(true),
            CtxAction::Rename => self.start_rename(&path),
            CtxAction::Pin => {
                let name = path.file_name().map_or_else(
                    || path.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if self.pin(&path) {
                    self.toast(format!("Pinned {name} to Quick access"));
                }
            }
            CtxAction::Unpin => {
                if self.unpin(&path) {
                    self.toast(String::from("Removed from Quick access"));
                }
            }
            CtxAction::Delete => self.delete_selection(false),
            CtxAction::DeleteForever => self.delete_selection(true),
        }
    }

    // ---- details pane -----------------------------------------------------

    /// The Explorer details pane: what the selection is, plus a preview of the
    /// one item under the cursor. Text shows its first lines, Markdown renders,
    /// images show the thumbnail.
    fn details_ui(&mut self, ui: &mut Ui) {
        let Some(path) = self.cursor_path() else {
            widgets::empty_state(ui, "No selection", "Pick a file to see its details");
            return;
        };
        let meta = std::fs::metadata(&path).ok();
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Name, in the same place the document header puts it.
        let (head, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
        let name_g = widgets::layout(ui, name, theme::bold_font(tfs::BODY), c::TEXT);
        let loc_g = widgets::layout_elided(
            ui,
            path.parent().unwrap_or(&path).to_string_lossy().to_string(),
            theme::ui_font(tfs::SMALL),
            c::TEXT_GHOST,
            (ui.available_width() - name_g.size().x - 40.0).max(40.0),
        );

        // Icon or thumbnail on top, then the facts.
        let preview_h = 168.0f32;
        let (art, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), preview_h), Sense::hover());
        let thumb = if is_dir {
            None
        } else {
            self.thumbs.get(&path, sp::THUMB_PX * 2)
        };

        let painter = ui.painter();
        widgets::galley_at(
            painter,
            Pos2::new(head.left(), head.center().y - name_g.size().y * 0.5),
            &name_g,
            c::TEXT,
        );
        let name_x = art.left() + 2.0;
        widgets::galley_at(
            painter,
            Pos2::new(name_x + name_g.size().x + 8.0, art.top() + 2.0),
            &loc_g,
            c::TEXT_GHOST,
        );
        match thumb {
            Some(tex) => {
                let src = tex.size_vec2();
                let scale = (preview_h * 0.8) / src.x.max(src.y);
                let size = src * scale;
                egui::Image::new(&tex)
                    .fit_to_exact_size(size)
                    .paint_at(ui, Rect::from_center_size(art.center(), size));
            }
            None => {
                let glyph = if is_dir {
                    (sp::ICON, c::TEXT)
                } else {
                    (56.0, c::TEXT_FAINT)
                };
                let rect = if is_dir {
                    Rect::from_min_size(Pos2::new(name_x, art.top() + 2.0), Vec2::splat(glyph.0))
                } else {
                    Rect::from_center_size(art.center(), Vec2::splat(glyph.0))
                };
                if is_dir {
                    Icon::Folder.paint(painter, rect, glyph.1);
                } else {
                    Icon::File.paint(painter, rect, glyph.1);
                }
            }
        }

        ui.add_space(sp::XS);
        properties_ui(ui, &path, self.measures.get(&path));

        // Text files get their first lines, the way Explorer's pane does.
        if !is_dir && fs_model::is_editable_text(&path) {
            ui.add_space(sp::SM);
            self.text_preview(ui, &path);
        }
    }

    /// Shows the head of a text file, read on a worker thread.
    fn text_preview(&mut self, ui: &mut Ui, path: &Path) {
        section_header(ui, "PREVIEW");
        let ready = self.peek.as_ref().is_some_and(|(p, _)| p == path);
        if !ready {
            if self.peek_pending.as_deref() != Some(path) {
                self.peek_pending = Some(path.to_path_buf());
                let p = path.to_path_buf();
                let tx = self.tx.clone();
                let _ = std::thread::Builder::new()
                    .name("xplor-peek".into())
                    .spawn(move || {
                        let _ = tx.send(Msg::Peek {
                            text: ops::peek_text(&p),
                            path: p,
                        });
                    });
            }
            widgets::empty_state(ui, "Reading\u{2026}", "First lines of the file");
            return;
        }
        let Some((_, text)) = self.peek.as_ref() else {
            return;
        };
        // Only the lines that fit; the rest is behind the editor.
        let width = ui.available_width();
        let line_h = theme::fs::MONO * 1.45;
        let room = (ui.available_height() / line_h).floor().max(1.0) as usize;
        let body: Vec<&str> = text.lines().take(room).collect();
        let g = widgets::layout_wrapped(
            ui,
            body.join("\n"),
            theme::mono_font(theme::fs::SMALL),
            c::TEXT_DIM,
            width,
        );
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, g.size().y + 2.0), Sense::hover());
        widgets::galley_at(ui.painter(), rect.min, &g, c::TEXT_DIM);
    }

    // ---- editor / preview pane --------------------------------------------

    fn doc_ui(&mut self, ui: &mut Ui) {
        if self.loading.is_some() {
            widgets::empty_state(ui, "Loading\u{2026}", "Reading the file");
            return;
        }
        if self.tab().is_some_and(|t| t.kind == TabKind::Folder) {
            widgets::empty_state(ui, "Files", "Open something to read it here");
            return;
        }
        let Some(doc) = self.doc() else {
            return;
        };
        let kind = doc.kind;
        let dirty = doc.dirty();
        let file_name = doc.file_name();
        let location = doc.location(&self.cwd);
        let external = doc.externally_changed;
        let read_only = doc.read_only;
        let is_md = kind == DocKind::Markdown;

        // ---- header
        //
        // Two rows instead of one crowded line: the title carries the weight,
        // the folder sits underneath in quiet grey, and the actions are
        // pictograms on the right. At 300px wide a single line could not hold
        // all three without the path collapsing to a single character.
        let title_h = 28.0f32;
        let path_h = 19.0f32;
        let header_h = title_h + path_h;
        let header =
            Rect::from_min_size(ui.min_rect().min, Vec2::new(ui.available_width(), header_h));
        let title_row = Rect::from_min_max(
            header.min,
            Pos2::new(header.right(), header.top() + title_h),
        );

        // Actions first, so the title knows how much room is left.
        let mut save = false;
        let mut wrap = false;
        let mut preview = false;
        let mut close = false;
        let btn = 26.0f32;
        let action_count = 3 + usize::from(is_md) + usize::from(dirty);
        let actions_w = btn * action_count as f32;
        {
            let spot = Rect::from_min_max(
                Pos2::new(title_row.right() - actions_w - sp::SM, title_row.top()),
                Pos2::new(title_row.right() - sp::SM, title_row.bottom()),
            );
            let mut bar = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(spot)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            // Close is always there; the rest appear only when they do something.
            let (cr, crr) = bar.allocate_exact_size(Vec2::splat(btn), Sense::click());
            if crr.hovered() {
                bar.painter()
                    .rect_filled(cr, CornerRadius::same(sp::RADIUS), c::HOVER);
            }
            Icon::Close.paint(
                bar.painter(),
                cr,
                if crr.hovered() { c::TEXT } else { c::TEXT_DIM },
            );
            if crr.on_hover_text("Close (Ctrl+W)").clicked() {
                close = true;
            }
            if is_md {
                let (r, rr) = bar.allocate_exact_size(Vec2::splat(btn), Sense::click());
                if rr.hovered() {
                    bar.painter()
                        .rect_filled(r, CornerRadius::same(sp::RADIUS), c::HOVER);
                }
                Icon::Preview.paint(
                    bar.painter(),
                    r,
                    if self.preview_visible {
                        c::ACCENT
                    } else if rr.hovered() {
                        c::TEXT
                    } else {
                        c::TEXT_DIM
                    },
                );
                if rr
                    .on_hover_text(if self.preview_visible {
                        "Hide the live preview"
                    } else {
                        "Show the live preview"
                    })
                    .clicked()
                {
                    preview = true;
                }
            }
            let (wr, wrr) = bar.allocate_exact_size(Vec2::splat(btn), Sense::click());
            if wrr.hovered() {
                bar.painter()
                    .rect_filled(wr, CornerRadius::same(sp::RADIUS), c::HOVER);
            }
            Icon::Wrap.paint(
                bar.painter(),
                wr,
                if self.wrap {
                    c::ACCENT
                } else if wrr.hovered() {
                    c::TEXT
                } else {
                    c::TEXT_DIM
                },
            );
            if wrr.on_hover_text("Soft wrap long lines").clicked() {
                wrap = true;
            }
            if dirty {
                let (sr, srr) = bar.allocate_exact_size(Vec2::splat(btn), Sense::click());
                if srr.hovered() {
                    bar.painter()
                        .rect_filled(sr, CornerRadius::same(sp::RADIUS), c::HOVER);
                }
                Icon::Save.paint(bar.painter(), sr, c::ACCENT);
                if srr.on_hover_text("Save (Ctrl+S)").clicked() {
                    save = true;
                }
            }
        }

        {
            let painter = ui.painter();
            painter.hline(
                header.left()..=header.right(),
                header.max.y - 0.5,
                Stroke::new(1.0, c::BORDER),
            );
            // Title: glyph, name, and a dot when there are unsaved changes.
            let x = header.left() + sp::SM;
            let cy = title_row.center().y;
            let icon = Rect::from_center_size(Pos2::new(x + 7.0, cy), Vec2::splat(15.0));
            let glyph_color = if dirty { c::TEXT_DIM } else { c::TEXT_GHOST };
            if is_md {
                Icon::Markdown.paint(painter, icon, glyph_color);
            } else {
                Icon::File.paint(painter, icon, glyph_color);
            }
            let name_x = x + 21.0;
            let name_room = (title_row.right() - actions_w - sp::SM - name_x - 10.0).max(24.0);
            let name_g = widgets::layout_elided(
                ui,
                file_name.clone(),
                if dirty {
                    theme::bold_font(tfs::BODY)
                } else {
                    theme::ui_font(tfs::BODY)
                },
                c::TEXT,
                name_room,
            );
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

            // Path underneath, elided in the middle so the folder name survives.
            let path_row = Rect::from_min_max(
                Pos2::new(header.left(), title_row.bottom()),
                Pos2::new(header.right(), header.bottom()),
            );
            let loc = widgets::layout_elided_middle(
                ui,
                location,
                theme::ui_font(tfs::SMALL),
                c::TEXT_FAINT,
                (path_row.width() - sp::SM * 2.0).max(24.0),
            );
            widgets::galley_at(
                painter,
                Pos2::new(
                    path_row.left() + sp::SM,
                    path_row.center().y - loc.size().y * 0.5,
                ),
                &loc,
                c::TEXT_FAINT,
            );
            if read_only {
                let g = widgets::layout(
                    ui,
                    "read only".to_owned(),
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
        if preview {
            self.preview_visible = !self.preview_visible;
        }
        if close {
            self.close_doc();
        }

        if save {
            self.save_doc();
        }
        if wrap {
            self.wrap = !self.wrap;
        }
        if preview {
            self.preview_visible = !self.preview_visible;
        }
        if close {
            self.close_doc();
        }

        // External change prompt.
        if external {
            let mut reload = false;
            let mut keep = false;
            Modal::new(Id::new("external_change"))
                .frame(theme::dialog_frame())
                .show(ui.ctx(), |ui| {
                    ui.set_width(360.0);
                    ui.label("This file changed on disk.");
                    ui.label(
                        egui::RichText::new("Another program or window saved it.")
                            .color(c::TEXT_FAINT)
                            .size(theme::fs::SMALL),
                    );
                    ui.add_space(sp::MD);
                    ui.horizontal(|ui| {
                        if ui.button("Reload from disk").clicked() {
                            reload = true;
                        }
                        if ui.button("Keep my version").clicked() {
                            keep = true;
                        }
                    });
                });
            if reload {
                if let Some(d) = self.doc_mut()
                    && let Err(e) = d.reload()
                {
                    self.toast_err(e);
                }
                self.preview.reset();
            }
            if keep {
                if let Some(d) = self.doc_mut() {
                    d.externally_changed = false;
                }
            }
        }

        let body = Rect::from_min_max(
            Pos2::new(ui.min_rect().left(), header.max.y),
            ui.max_rect().max,
        );
        if is_md && self.preview_visible {
            self.split_ui(ui, body);
        } else {
            self.editor_ui(ui, body);
        }
    }

    /// Editor left, live preview right, with a draggable divider.
    ///
    /// Each pane gets its own `Ui` anchored to an explicit rect. Note
    /// `new_child`, not `scope_builder`: the latter advances its parent's
    /// cursor, which would push the second pane off the bottom of the panel.
    fn split_ui(&mut self, ui: &mut Ui, rect: Rect) {
        let divider_w = 7.0f32;
        let usable = (rect.width() - divider_w).max(200.0);
        let left_w = (usable * self.split).clamp(160.0, usable);
        let left = Rect::from_min_size(rect.min, Vec2::new(left_w, rect.height()));
        let divider = Rect::from_min_size(
            Pos2::new(rect.min.x + left_w, rect.min.y),
            Vec2::new(divider_w, rect.height()),
        );
        let right = Rect::from_min_max(Pos2::new(divider.max.x, rect.min.y), rect.max);

        let mut left_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(left)
                .layout(egui::Layout::top_down(egui::Align::LEFT))
                .id_salt("editor-pane"),
        );
        self.editor_ui(&mut left_ui, left);

        // The divider is a real widget, so the pointer can grab and drag it.
        let resp = ui.interact(
            divider,
            Id::new("split"),
            Sense::hover().union(Sense::drag()),
        );
        if ui.is_rect_visible(divider) {
            if resp.hovered() || resp.dragged() {
                ui.painter()
                    .rect_filled(divider, CornerRadius::ZERO, c::HOVER);
            }
            // A hairline seam, so the split reads even when unhovered.
            ui.painter().vline(
                divider.center().x,
                divider.y_range(),
                egui::Stroke::new(1.0, c::DIVIDER),
            );
        }
        if resp.dragged() {
            self.split = ((left_w + resp.drag_delta().x) / usable).clamp(0.2, 0.8);
        }

        let mut right_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right)
                .layout(egui::Layout::top_down(egui::Align::LEFT))
                .id_salt("preview-pane"),
        );
        self.preview_ui(&mut right_ui, right);
    }

    /// The code editor.
    fn editor_ui(&mut self, ui: &mut Ui, rect: Rect) {
        let Some((path, before_chars)) =
            self.doc().map(|d| (d.path.clone(), d.text.chars().count()))
        else {
            return;
        };
        let wrap = self.wrap;
        let syntax = editor::syntax_for(&path);

        // These keys are claimed before the text widget gets them.
        let (tab, shift_tab) = ui.ctx().input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::Tab),
                i.consume_key(egui::Modifiers::SHIFT, Key::Tab),
            )
        });
        let comment = ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::CTRL, Key::Slash))
            || ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Slash));

        // Typing the closer of a pair we just opened steps past it instead of
        // inserting a duplicate. The caret is known from the previous frame.
        let mut skip_to: Option<usize> = None;
        if let Some(caret) = self.editor_caret {
            let ready = self
                .doc()
                .is_some_and(|d| editing::should_skip_closer(&d.text, caret));
            if ready
                && ui
                    .ctx()
                    .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::CloseBracket))
            {
                skip_to = Some(caret + 1);
            }
        }

        let (output, _tokens) = {
            let doc = self.doc_mut().expect("checked above");
            let child = ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT))
                    .id_salt("editor"),
                |ui| {
                    let mut ce = egui_code_editor::CodeEditor::default()
                        .id_source(EDITOR_ID)
                        .with_theme(editor_theme())
                        .with_fontsize(theme::fs::MONO)
                        .with_numlines(true)
                        .with_numlines_only_natural(true)
                        .with_numlines_shift(-1)
                        .with_wrap(wrap)
                        .with_clickable_links(true)
                        .with_rows(14);
                    ce.show(ui, &mut doc.text as &mut dyn egui::TextBuffer, &syntax)
                },
            );
            child.inner
        };

        let id = output.response.id;
        let range = output.cursor_range;
        self.editor_caret = range.map(|r| r.primary.index.0);
        let after_chars = self.doc().map_or(0, |d| d.text.chars().count());
        let typed_one = after_chars == before_chars + 1;
        if let Some(target) = skip_to {
            self.set_text_cursor(ui.ctx(), id, target);
        } else if typed_one {
            self.auto_format(ui.ctx(), &path, id, range);
        } else if tab || shift_tab {
            self.indent_lines(ui.ctx(), id, range, shift_tab);
        } else if comment {
            self.toggle_comment(ui.ctx(), &path, id, range);
        }
    }

    /// Markdown preview, refreshed after a short pause in typing.
    fn preview_ui(&mut self, ui: &mut Ui, _rect: Rect) {
        let Some(doc) = self.doc() else {
            return;
        };
        if let Some(last) = self.last_edit {
            if last.elapsed() >= PREVIEW_DEBOUNCE {
                self.render_version = doc.version;
                self.last_edit = None;
            }
        } else {
            self.render_version = doc.version;
        }
        let version = self.render_version;
        let text = self.doc().map(|d| d.text.clone()).unwrap_or_default();
        self.preview.sync(&text, version);

        let indent = sp::LG;
        // The `Ui` we were handed is already anchored to `rect`, so the scroll
        // area simply fills it.
        egui::ScrollArea::vertical()
            .id_salt("preview")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let height = self.preview.show(ui, indent);
                ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), height + sp::XL),
                    Sense::hover(),
                );
            });
    }

    // ---- status bar --------------------------------------------------------

    fn status_ui(&mut self, ui: &mut Ui) {
        let rect = Rect::from_min_size(
            ui.min_rect().min,
            Vec2::new(ui.available_width(), sp::STATUS),
        );
        let searching = self.searching();
        let total = self.entries.len();
        let shown = self.row_count();

        let left = if searching {
            format!("{shown} results")
        } else if !self.filter.is_empty() {
            format!("{shown} of {total}")
        } else if total == 1 {
            "1 item".to_owned()
        } else {
            format!("{total} items")
        };
        let center = if self.sel.is_empty() {
            if searching && self.search.scanned > 0 {
                format!("{} scanned", self.search.scanned)
            } else {
                String::new()
            }
        } else {
            // Folders are measured on a worker, so the total is the sum of
            // what is known: exact once every folder has reported, short by
            // the rest while they are still being walked. Never block here.
            let mut bytes = 0u64;
            let mut pending = 0usize;
            for p in &self.sel {
                if p.is_dir() {
                    match self.measures.get(p) {
                        Some(m) => bytes = bytes.saturating_add(m.bytes),
                        None => pending += 1,
                    }
                } else {
                    bytes = bytes.saturating_add(p.metadata().map(|m| m.len()).unwrap_or(0));
                }
            }
            let n = self.sel.len();
            let mark = if pending > 0 { "+\u{2026}" } else { "" };
            format!(
                "{n} selected  \u{00B7}  {}{mark}",
                fs_model::fmt_size(bytes)
            )
        };

        let right = self.jobs.first().map(|j| {
            let frac = if j.total_bytes > 0 {
                j.done_bytes as f32 / j.total_bytes as f32
            } else if j.total_items > 0 {
                j.done_items as f32 / j.total_items as f32
            } else {
                0.0
            };
            (j.job.label.clone(), frac)
        });
        widgets::status_bar(
            ui,
            rect,
            &left,
            &center,
            right.as_ref().map(|(l, f)| (l.as_str(), *f)),
        );

        // A running job gets a cancel affordance in the status bar.
        let mut cancel_job = None;
        if !self.jobs.is_empty() {
            let spot = Rect::from_min_max(
                Pos2::new(rect.right() - 200.0, rect.top()),
                Pos2::new(rect.right() - sp::SM, rect.bottom()),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(spot), |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::flat_button(ui, "Cancel", "Stop this operation").clicked() {
                        cancel_job = self.jobs.first().map(|j| j.job.id);
                    }
                });
            });
        }
        if let Some(id) = cancel_job
            && let Some(j) = self.jobs.iter_mut().find(|j| j.job.id == id)
        {
            j.job
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }

        let painter = ui.painter();
        // Right side: current file, then timing, in quiet grey.
        let mut right_x = rect.right() - sp::SM;
        if let Some(j) = self.jobs.first() {
            let g = widgets::layout_elided(
                ui,
                j.current.clone(),
                theme::ui_font(tfs::SMALL),
                c::TEXT_GHOST,
                150.0,
            );
            widgets::text_right(
                painter,
                Pos2::new(right_x, rect.center().y),
                &g,
                c::TEXT_GHOST,
            );
            right_x -= 158.0;
        }
        let info = match self.doc() {
            Some(d) => {
                let stats = self.preview.stats();
                let kb = d.text.len() as f32 / 1024.0;
                // Kept short on purpose: the status bar has a view switch to
                // its right, and an elided number helps nobody.
                format!(
                    "{kb:.1} KB  \u{00B7}  {:.0} us parse  \u{00B7}  {:.1} ms",
                    stats.parse_us as f32, self.perf.update_ms
                )
            }
            None => format!("{:.1} ms", self.perf.update_ms),
        };
        let g = widgets::layout_elided(ui, info, theme::ui_font(tfs::SMALL), c::TEXT_GHOST, 210.0);
        widgets::text_right(
            painter,
            Pos2::new(right_x, rect.center().y),
            &g,
            c::TEXT_GHOST,
        );
        right_x -= 208.0;

        // View switch, bottom right, the way Explorer has it. Three small
        // pictograms instead of words, so it costs about 70 pixels.
        let seg_w = 22.0f32;
        let segs = ViewMode::ALL.len() as f32 * seg_w;
        let segs_rect = Rect::from_min_max(
            Pos2::new(right_x - segs, rect.top() + 4.0),
            Pos2::new(right_x, rect.bottom() - 4.0),
        );
        if rect.width() > 560.0 {
            let mut picked = None;
            for (i, mode) in ViewMode::ALL.iter().enumerate() {
                let seg = Rect::from_min_size(
                    Pos2::new(segs_rect.left() + i as f32 * seg_w, segs_rect.top()),
                    Vec2::new(seg_w, segs_rect.height()),
                );
                let resp = ui.interact(seg, Id::new(("view", mode.label())), Sense::click());
                if ui.is_rect_visible(seg) {
                    let active = *mode == self.view;
                    if active || resp.hovered() {
                        ui.painter().rect_filled(
                            seg,
                            CornerRadius::same(3),
                            if active { c::SEL } else { c::HOVER },
                        );
                    }
                    let color = if active { c::SEL_TEXT } else { c::TEXT_DIM };
                    let center = seg.center();
                    match mode {
                        ViewMode::Details => {
                            // Three short lines: the column metaphor.
                            for n in 0..3 {
                                let y = center.y - 5.0 + n as f32 * 4.0;
                                ui.painter().hline(
                                    center.x - 6.0..=center.x + 6.0,
                                    y,
                                    egui::Stroke::new(1.0, color),
                                );
                            }
                        }
                        ViewMode::List => {
                            for n in 0..3 {
                                let y = center.y - 5.0 + n as f32 * 4.0;
                                ui.painter().rect_filled(
                                    Rect::from_min_size(
                                        Pos2::new(center.x - 6.0, y - 1.0),
                                        Vec2::new(2.0, 2.0),
                                    ),
                                    0.0,
                                    color,
                                );
                                ui.painter().hline(
                                    center.x - 2.0..=center.x + 6.0,
                                    y,
                                    egui::Stroke::new(1.0, color),
                                );
                            }
                        }
                        ViewMode::Large => {
                            // Four squares: the tile metaphor.
                            for n in 0..4 {
                                ui.painter().rect_stroke(
                                    Rect::from_center_size(
                                        center
                                            + Vec2::new(
                                                if n % 2 == 0 { -3.5 } else { 3.5 },
                                                if n < 2 { -3.5 } else { 3.5 },
                                            ),
                                        Vec2::splat(5.0),
                                    ),
                                    1.0,
                                    egui::Stroke::new(1.0, color),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }
                    }
                }
                if resp.clicked() {
                    picked = Some(*mode);
                }
                let _ = resp.on_hover_text(mode.hint());
            }
            if let Some(mode) = picked {
                self.set_view(mode);
            }
        }
    }

    /// Switches the list layout and forgets anything sized to the old one.
    fn set_view(&mut self, mode: ViewMode) {
        if self.view == mode {
            return;
        }
        self.view = mode;
        self.row_cache.clear();
    }

    // ---- dialogs -----------------------------------------------------------

    fn dialogs(&mut self, ctx: &Context) {
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let next = match dialog {
            Dialog::None => None,
            Dialog::Rename { path, name } => {
                let mut name = name;
                let mut ok = false;
                let mut cancel = false;
                self.field_dialog(
                    ctx,
                    "rename_dlg",
                    "Rename",
                    &mut name,
                    "Rename",
                    "Cancel",
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    self.apply_rename(&path, &name);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::Rename { path, name })
                }
            }
            Dialog::Create { dir, name, folder } => {
                let mut name = name;
                let mut ok = false;
                let mut cancel = false;
                self.field_dialog(
                    ctx,
                    "create_dlg",
                    if folder { "New folder" } else { "New file" },
                    &mut name,
                    "Create",
                    "Cancel",
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    self.apply_create(&dir, &name, folder);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::Create { dir, name, folder })
                }
            }
            Dialog::Path { text } => {
                let mut text = text;
                let mut ok = false;
                let mut cancel = false;
                self.field_dialog(
                    ctx,
                    "path_dlg",
                    "Go to folder",
                    &mut text,
                    "Go",
                    "Cancel",
                    &mut ok,
                    &mut cancel,
                );
                if ok {
                    match fs_model::resolve_input(&text, &self.cwd) {
                        Some(p) if p.is_dir() => {
                            self.navigate(&p);
                            None
                        }
                        Some(p) => {
                            self.toast_err(format!("{} is not a folder", p.display()));
                            Some(Dialog::Path { text })
                        }
                        None => Some(Dialog::Path { text }),
                    }
                } else if cancel {
                    None
                } else {
                    Some(Dialog::Path { text })
                }
            }
            Dialog::ConfirmDelete { paths } => {
                let mut ok = false;
                let mut cancel = false;
                Modal::new(Id::new("confirm_delete"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        let what = if paths.len() == 1 {
                            format!("\u{201C}{}\u{201D}", display_name(&paths[0]))
                        } else {
                            format!("{} items", paths.len())
                        };
                        ui.label(format!("Permanently delete {what}?"));
                        ui.label(
                            egui::RichText::new("This cannot be undone.")
                                .color(c::TEXT_FAINT)
                                .size(theme::fs::SMALL),
                        );
                        ui.add_space(sp::MD);
                        ui.horizontal(|ui| {
                            if ui.button("Delete permanently").clicked() {
                                ok = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if ok {
                    self.start_permanent_delete(paths);
                    None
                } else if cancel {
                    None
                } else {
                    Some(Dialog::ConfirmDelete { paths })
                }
            }
            Dialog::Unsaved { path, close_app } => {
                let mut save = false;
                let mut discard = false;
                let mut cancel = false;
                Modal::new(Id::new("unsaved"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(380.0);
                        ui.label(format!(
                            "Save changes to \u{201C}{}\u{201D}?",
                            display_name(&path)
                        ));
                        ui.add_space(sp::MD);
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                save = true;
                            }
                            if ui.button("Discard").clicked() {
                                discard = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                        });
                    });
                if save || discard {
                    if save {
                        self.save_doc();
                    }
                    // The prompt came from a tab cross, or from Ctrl+W on the
                    // active tab; either way close the tab it named.
                    match self.pending_close.take() {
                        Some(i) if i < self.tabs.len() => self.close_tab(i),
                        _ => self.close_active_tab(),
                    }
                    if close_app {
                        self.close_armed = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    None
                } else if cancel {
                    self.pending_close = None;
                    None
                } else {
                    Some(Dialog::Unsaved { path, close_app })
                }
            }
            Dialog::Properties { path } => {
                let mut close = false;
                Modal::new(Id::new("properties"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        properties_ui(ui, &path, self.measures.get(&path));
                        ui.add_space(sp::MD);
                        if ui.button("OK").clicked() {
                            close = true;
                        }
                    });
                if !close {
                    Some(Dialog::Properties { path })
                } else {
                    None
                }
            }
            Dialog::Help => {
                let mut close = false;
                Modal::new(Id::new("help"))
                    .frame(theme::dialog_frame())
                    .show(ctx, |ui| {
                        ui.set_width(460.0);
                        ui.label(egui::RichText::new("Shortcuts").strong());
                        ui.add_space(sp::SM);
                        let rows: &[(&str, &str)] = &[
                            ("Enter  /  double-click", "Open"),
                            ("Backspace  /  Alt+Up", "Up one folder"),
                            ("Alt+Left  /  Alt+Right", "Back  /  forward"),
                            ("Ctrl+L", "Go to folder"),
                            ("Ctrl+F", "Focus the search box"),
                            ("Ctrl+1  /  2  /  3", "Details  /  list  /  large icons"),
                            ("Ctrl+Shift+V", "Cycle the view"),
                            ("Ctrl+B", "Toggle the sidebar"),
                            ("Alt+P", "Toggle the details pane"),
                            ("Alt+Enter", "Properties"),
                            ("Ctrl+H", "Show hidden items"),
                            ("F5", "Reload the folder"),
                            ("Ctrl+A", "Select all"),
                            ("Space", "Toggle the row under the cursor"),
                            ("Shift+Up  /  Shift+Down", "Extend selection"),
                            ("Ctrl+C  /  Ctrl+X  /  Ctrl+V", "Copy  /  cut  /  paste"),
                            ("Delete", "Move to trash"),
                            ("Shift+Delete", "Delete permanently"),
                            ("Ctrl+Z", "Undo the last file operation"),
                            ("F2", "Rename"),
                            ("Ctrl+N  /  Ctrl+Shift+N", "New document  /  folder"),
                            ("Alt+F", "The New menu, including compress to ZIP"),
                            ("Ctrl+S  /  Ctrl+W", "Save  /  close the tab"),
                            ("Ctrl+T", "New tab showing files"),
                            ("Ctrl+Tab  /  Ctrl+Shift+Tab", "Next  /  previous tab"),
                            ("Tab  /  Shift+Tab", "Indent  /  outdent"),
                            ("Ctrl+/", "Toggle a line comment"),
                            ("Escape", "Clear the search, then the selection"),
                            ("?  /  /", "This list"),
                        ];
                        egui::Grid::new("help_grid")
                            .num_columns(2)
                            .spacing([sp::LG, sp::XS])
                            .show(ui, |ui| {
                                for (k, v) in rows {
                                    ui.label(egui::RichText::new(*k).monospace().color(c::TEXT));
                                    ui.label(
                                        egui::RichText::new(*v)
                                            .color(c::TEXT_FAINT)
                                            .size(theme::fs::SMALL),
                                    );
                                    ui.end_row();
                                }
                            });
                        ui.add_space(sp::MD);
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                if close { None } else { Some(Dialog::Help) }
            }
        };
        self.dialog = next.unwrap_or(Dialog::None);
    }

    /// A single-field dialog, with live validation.
    #[allow(clippy::too_many_arguments)]
    fn field_dialog(
        &mut self,
        ctx: &Context,
        id: &str,
        title: &str,
        value: &mut String,
        confirm: &str,
        cancel: &str,
        ok: &mut bool,
        cancelled: &mut bool,
    ) {
        Modal::new(Id::new(id))
            .frame(theme::dialog_frame())
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(egui::RichText::new(title).strong());
                ui.add_space(sp::SM);
                let mut typed = false;
                let out = TextEdit::singleline(value)
                    .id(Id::new((id, "field")))
                    .desired_width(f32::INFINITY)
                    .frame(
                        Frame::new()
                            .fill(c::CODE_BG)
                            .stroke(Stroke::new(1.0, c::BORDER))
                            .corner_radius(CornerRadius::same(sp::RADIUS))
                            .inner_margin(Margin::symmetric(sp::SM_I, 6)),
                    )
                    .show(ui);
                typed |= out.response.changed();

                let check_name = !id.contains("path");
                let validation = if check_name {
                    fs_model::validate_name(value)
                } else {
                    Ok(())
                };
                if let Err(msg) = &validation {
                    ui.label(
                        egui::RichText::new(msg)
                            .color(c::DANGER)
                            .size(theme::fs::SMALL),
                    );
                }
                let enabled = validation.is_ok() && !value.trim().is_empty();
                ui.add_space(sp::MD);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(enabled, egui::Button::new(confirm))
                        .clicked()
                    {
                        *ok = true;
                    }
                    if ui.button(cancel).clicked() {
                        *cancelled = true;
                    }
                });
                // Keep focus in the field as the dialog persists across frames.
                if typed {
                    out.response.request_focus();
                }
            });
    }

    /// Transient messages, stacked above the status bar on the right.
    fn draw_toasts(&mut self, ctx: &Context) {
        if self.toasts.is_empty() {
            return;
        }
        let now = Instant::now();
        let items: Vec<(String, bool, f32)> = self
            .toasts
            .iter()
            .map(|t| {
                (
                    t.text.clone(),
                    t.danger,
                    now.duration_since(t.born).as_secs_f32() / TOAST_TTL.as_secs_f32(),
                )
            })
            .collect();
        let width = 320.0f32;
        let height = 30.0 * items.len() as f32 + sp::XS * (items.len() as f32 - 1.0);
        let screen = ctx.input(|i| i.viewport_rect());
        let pos = Pos2::new(
            (screen.right() - width - sp::SM).max(screen.left() + sp::SM),
            (screen.bottom() - sp::STATUS - sp::SM - height).max(screen.top() + sp::TITLE),
        );
        egui::Area::new(Id::new("toasts"))
            .fixed_pos(pos)
            .interactable(false)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::new(0.0, sp::XS);
                for (text, danger, age) in items {
                    let alpha = (1.0 - age * age).clamp(0.0, 1.0);
                    let a = |col: Color32| {
                        Color32::from_rgba_premultiplied(
                            col.r(),
                            col.g(),
                            col.b(),
                            (alpha * 255.0) as u8,
                        )
                    };
                    let (rect, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), a(c::RAISED));
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(sp::RADIUS),
                        Stroke::new(1.0, a(if danger { c::DANGER } else { c::BORDER })),
                        StrokeKind::Inside,
                    );
                    let color = if danger { c::DANGER } else { c::TEXT_DIM };
                    let g = widgets::layout_elided(
                        ui,
                        text,
                        theme::ui_font(tfs::SMALL),
                        color,
                        ui.available_width() - 20.0,
                    );
                    widgets::galley_at(
                        painter,
                        Pos2::new(rect.left() + sp::SM, rect.center().y - g.size().y * 0.5),
                        &g,
                        a(color),
                    );
                }
            });
    }

    // ---- keyboard ------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &Context) {
        let mut k = Keys::default();
        ctx.input_mut(|i| {
            k.help = i.consume_key(egui::Modifiers::NONE, Key::Slash);
            k.alt_left = i.consume_key(egui::Modifiers::ALT, Key::ArrowLeft);
            k.alt_right = i.consume_key(egui::Modifiers::ALT, Key::ArrowRight);
            k.alt_up = i.consume_key(egui::Modifiers::ALT, Key::ArrowUp);
            k.go_path = i.consume_key(egui::Modifiers::CTRL, Key::L)
                || i.consume_key(egui::Modifiers::COMMAND, Key::L);
            k.focus_search = i.consume_key(egui::Modifiers::CTRL, Key::F)
                || i.consume_key(egui::Modifiers::COMMAND, Key::F);
            k.refresh = i.consume_key(egui::Modifiers::NONE, Key::F5);
            k.toggle_hidden = i.consume_key(egui::Modifiers::CTRL, Key::H)
                || i.consume_key(egui::Modifiers::COMMAND, Key::H);
            k.toggle_sidebar = i.consume_key(egui::Modifiers::CTRL, Key::B)
                || i.consume_key(egui::Modifiers::COMMAND, Key::B);
            k.close_file = i.consume_key(egui::Modifiers::CTRL, Key::W)
                || i.consume_key(egui::Modifiers::COMMAND, Key::W);
            k.new_tab = i.consume_key(egui::Modifiers::CTRL, Key::T)
                || i.consume_key(egui::Modifiers::COMMAND, Key::T);
            k.next_tab = i.consume_key(egui::Modifiers::CTRL, Key::Tab)
                || i.consume_key(egui::Modifiers::CTRL, Key::PageDown);
            k.prev_tab = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Tab)
                || i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::PageUp);
            k.new_file = i.consume_key(egui::Modifiers::CTRL, Key::N);
            k.new_folder = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::N);
            k.save = i.consume_key(egui::Modifiers::CTRL, Key::S)
                || i.consume_key(egui::Modifiers::COMMAND, Key::S);
            k.select_all = i.consume_key(egui::Modifiers::CTRL, Key::A)
                || i.consume_key(egui::Modifiers::COMMAND, Key::A);
            k.copy = i.consume_key(egui::Modifiers::CTRL, Key::C)
                || i.consume_key(egui::Modifiers::COMMAND, Key::C);
            k.cut = i.consume_key(egui::Modifiers::CTRL, Key::X)
                || i.consume_key(egui::Modifiers::COMMAND, Key::X);
            k.paste = i.consume_key(egui::Modifiers::CTRL, Key::V)
                || i.consume_key(egui::Modifiers::COMMAND, Key::V);
            k.rename = i.consume_key(egui::Modifiers::NONE, Key::F2);
            k.delete_forever = i.consume_key(egui::Modifiers::SHIFT, Key::Delete);
            k.delete = i.consume_key(egui::Modifiers::NONE, Key::Delete);
            k.undo = i.consume_key(egui::Modifiers::CTRL, Key::Z)
                || i.consume_key(egui::Modifiers::COMMAND, Key::Z);
            k.properties = i.consume_key(egui::Modifiers::ALT, Key::Enter);
            k.new_menu = i.consume_key(egui::Modifiers::ALT, Key::F);
            k.toggle_details = i.consume_key(egui::Modifiers::ALT, Key::P);
            k.escape = i.consume_key(egui::Modifiers::NONE, Key::Escape);
            // Ctrl+1..3 pick the list layout, the way Explorer does.
            for (digit, key) in [(1u8, Key::Num1), (2, Key::Num2), (3, Key::Num3)] {
                if i.consume_key(egui::Modifiers::CTRL, key)
                    || i.consume_key(egui::Modifiers::COMMAND, key)
                {
                    k.view = ViewMode::from_digit(digit);
                }
            }
            if i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::V)
                || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::V)
            {
                k.cycle_view = true;
            }
        });
        k.search_go = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));

        if k.help {
            self.dialog = if matches!(self.dialog, Dialog::Help) {
                Dialog::None
            } else {
                Dialog::Help
            };
        }
        if k.alt_left {
            self.go_back();
        }
        if k.alt_right {
            self.go_forward();
        }
        if k.alt_up {
            self.go_up();
        }
        if k.go_path {
            self.dialog = Dialog::Path {
                text: self.cwd.to_string_lossy().to_string(),
            };
        }
        if k.focus_search {
            self.search_focus = true;
        }
        if k.refresh {
            self.request_listing();
        }
        if k.toggle_hidden {
            self.show_hidden = !self.show_hidden;
            self.request_listing();
        }
        if k.toggle_sidebar {
            self.sidebar = !self.sidebar;
        }
        if k.new_menu {
            // Consumed by the toolbar's New button, which owns the popup id.
            self.new_menu = true;
        }
        if k.toggle_details {
            self.details = !self.details;
        }
        if k.close_file {
            self.close_doc();
        }
        if k.new_tab {
            self.new_tab();
        }
        if k.next_tab {
            self.cycle_tab(false);
        }
        if k.prev_tab {
            self.cycle_tab(true);
        }
        if k.new_file {
            self.dialog = Dialog::Create {
                dir: self.cwd.clone(),
                name: String::new(),
                folder: false,
            };
        }
        if k.new_folder {
            self.dialog = Dialog::Create {
                dir: self.cwd.clone(),
                name: String::new(),
                folder: true,
            };
        }
        if k.save {
            self.save_doc();
        }
        if k.select_all {
            self.select_all();
        }
        if k.copy {
            self.copy_selection(false);
        }
        if k.cut {
            self.copy_selection(true);
        }
        if k.paste {
            self.paste();
        }
        if k.rename {
            if let Some(p) = self.cursor_path() {
                self.start_rename(&p);
            }
        }
        if k.delete_forever {
            self.delete_selection(true);
        }
        if k.delete {
            self.delete_selection(false);
        }
        if k.undo {
            self.undo();
        }
        if k.properties {
            if let Some(p) = self.cursor_path() {
                self.dialog = Dialog::Properties { path: p };
            }
        }
        if let Some(mode) = k.view {
            self.set_view(mode);
        }
        if k.cycle_view {
            let next = self.view.next();
            self.set_view(next);
        }
        if k.escape {
            if !self.filter.is_empty() {
                self.filter.clear();
                self.clear_search();
                self.recompute_visible();
            } else if !self.sel.is_empty() {
                self.sel.clear();
            } else if self.searching() {
                self.clear_search();
            }
        }
        if k.search_go && self.filter_focused(ctx) {
            // Typing already searches; Enter just refreshes it immediately.
            self.pump_search();
        }

        // List navigation, but only when nothing else wants the keyboard.
        if !ctx.egui_wants_keyboard_input() && matches!(self.dialog, Dialog::None) {
            self.list_keys(ctx);
        }

        // Files dropped from the OS.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.first().map(|f| f.path().to_path_buf()) {
            if path.is_dir() {
                self.navigate(&path);
            } else {
                if let Some(parent) = path.parent() {
                    self.navigate(parent);
                }
                self.open_path(&path);
            }
        }
    }

    fn list_keys(&mut self, ctx: &Context) {
        let count = self.row_count();
        if count == 0 {
            return;
        }
        let page = 12isize;
        let mut nav = (
            false, false, false, false, false, false, false, false, false,
        );
        ctx.input_mut(|i| {
            nav.0 = i.consume_key(egui::Modifiers::NONE, Key::ArrowUp);
            nav.1 = i.consume_key(egui::Modifiers::NONE, Key::ArrowDown);
            nav.2 = i.consume_key(egui::Modifiers::NONE, Key::Home);
            nav.3 = i.consume_key(egui::Modifiers::NONE, Key::End);
            nav.4 = i.consume_key(egui::Modifiers::NONE, Key::PageUp);
            nav.5 = i.consume_key(egui::Modifiers::NONE, Key::PageDown);
            nav.6 = i.consume_key(egui::Modifiers::NONE, Key::Space);
            nav.7 = i.consume_key(egui::Modifiers::NONE, Key::Enter);
            nav.8 = i.consume_key(egui::Modifiers::NONE, Key::Backspace);
        });
        let shift = ctx.input(|i| i.modifiers.shift);
        let step = |cur: usize, delta: isize| -> usize {
            (cur as isize + delta).clamp(0, count as isize - 1) as usize
        };

        if nav.0 {
            let next = step(self.cursor, -1);
            self.move_cursor(next, shift);
        }
        if nav.1 {
            let next = step(self.cursor, 1);
            self.move_cursor(next, shift);
        }
        if nav.4 {
            let next = step(self.cursor, -page);
            self.move_cursor(next, shift);
        }
        if nav.5 {
            let next = step(self.cursor, page);
            self.move_cursor(next, shift);
        }
        if nav.2 {
            self.move_cursor(0, shift);
        }
        if nav.3 {
            self.move_cursor(count - 1, shift);
        }
        if nav.6 {
            if let Some(p) = self.cursor_path() {
                if !self.sel.remove(&p) {
                    self.sel.insert(p);
                }
                self.set_cursor(step(self.cursor, 1));
            }
        }
        if nav.7 {
            if let Some(p) = self.cursor_path() {
                self.open_path(&p);
            }
        }
        if nav.8 {
            self.go_up();
        }

        // Typing a letter jumps to the next row whose name starts with it,
        // the way Explorer does. Anything that moves the cursor resets it.
        if nav.0 || nav.1 || nav.2 || nav.3 || nav.4 || nav.5 || nav.6 {
            self.typeahead.clear();
        }
        self.typeahead_keys(ctx, count);
    }

    /// Feeds plain letter keys to the type-ahead buffer and jumps on a match.
    fn typeahead_keys(&mut self, ctx: &Context, count: usize) {
        let mut typed = None;
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = ev
                    && modifiers.is_none()
                {
                    typed = typeahead::typed_char(*key);
                }
            }
        });
        let Some(ch) = typed else { return };

        self.typeahead.push(ch, Instant::now());
        // Only visible rows can be jumped to, so build just those names.
        let names: Vec<String> = (0..count)
            .filter_map(|i| self.path_at(i))
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect();
        if let Some(next) = self.typeahead.find(&names, self.cursor) {
            self.move_cursor(next, false);
        }
    }

    /// Moves the cursor, carrying the selection with it unless Shift is held.
    fn move_cursor(&mut self, next: usize, extend: bool) {
        if extend {
            let lo = self.anchor.min(next);
            let hi = self.anchor.max(next);
            self.sel.clear();
            for k in lo..=hi {
                if let Some(p) = self.path_at(k) {
                    self.sel.insert(p);
                }
            }
        } else {
            // The selection follows the cursor, so it must be the *new* row.
            self.sel.clear();
            if let Some(p) = self.path_at(next) {
                self.sel.insert(p);
            }
        }
        self.set_cursor(next);
    }

    fn set_cursor(&mut self, next: usize) {
        self.cursor = next;
        self.anchor = next;
        let align = if next == 0 {
            Align2::LEFT_TOP
        } else {
            Align2::CENTER_CENTER
        };
        self.scroll_to = Some((next, align));
    }

    fn request_close(&mut self, ctx: &Context) {
        if let Some(i) = self.unsaved_tab() {
            // Name the file that actually needs saving, not the visible one.
            self.pending_close = Some(i);
            let path = self.tabs[i].doc.path.clone();
            self.dialog = Dialog::Unsaved {
                path,
                close_app: true,
            };
        } else {
            self.close_armed = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn close_guard(&mut self, ctx: &Context) {
        let closing = ctx.input(|i| i.viewport().close_requested());
        if !closing {
            return;
        }
        if let Some(i) = self.unsaved_tab() {
            if !self.close_armed {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.pending_close = Some(i);
                let path = self.tabs[i].doc.path.clone();
                self.dialog = Dialog::Unsaved {
                    path,
                    close_app: true,
                };
            }
        } else {
            self.close_armed = true;
        }
    }

    /// The first tab with unsaved changes, for the "save before closing" prompt.
    fn unsaved_tab(&self) -> Option<usize> {
        self.tabs.first_dirty()
    }

    /// Closes the tab on screen, used by `Ctrl+W` and the tab strip.
    fn close_active_tab(&mut self) {
        if self.has_tabs() {
            let active = self.tabs.active;
            self.close_tab(active);
        }
    }

    /// The row of open documents above the editor.
    ///
    /// Tabs share the width evenly and elide their names, so a long path can
    /// never push the row wider than the panel. Clicking focuses, the little
    /// cross closes, and the middle button closes too.
    fn tab_strip(&mut self, ui: &mut Ui, rect: Rect) {
        let count = self.tabs.len();
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.max.y - 0.5,
            Stroke::new(1.0, c::BORDER),
        );
        if count == 0 {
            return;
        }
        // Tabs never get narrower than this; the row scrolls instead.
        const MIN_W: f32 = 96.0;
        const MAX_W: f32 = 190.0;
        let w = ((rect.width() - 4.0) / count as f32).clamp(MIN_W, MAX_W);
        let close_w = 16.0f32;
        let mut focus = None;
        let mut close = None;

        for (i, tab) in self.tabs.iter().enumerate() {
            let x = rect.left() + 2.0 + i as f32 * w;
            if x + w > rect.right() {
                break;
            }
            let tab_rect = Rect::from_min_size(
                Pos2::new(x, rect.top() + 2.0),
                Vec2::new(w - 2.0, rect.height() - 3.0),
            );
            let resp = ui.interact(tab_rect, Id::new(("tab", i)), Sense::click());
            if resp.clicked() {
                focus = Some(i);
            }
            if resp.middle_clicked() {
                close = Some(i);
            }

            let active = i == self.tabs.active;
            let dirty = tab.doc.dirty();
            let painter = ui.painter();
            if active {
                painter.rect_filled(tab_rect, CornerRadius::same(sp::RADIUS), c::SEL);
            } else if resp.hovered() {
                painter.rect_filled(tab_rect, CornerRadius::same(sp::RADIUS), c::HOVER);
            }
            // The active tab is joined to the body below it by a hairline.
            if active {
                painter.hline(
                    tab_rect.left()..=tab_rect.right(),
                    tab_rect.max.y - 0.5,
                    Stroke::new(1.0, c::BG),
                );
            }

            let text_color = if active { c::SEL_TEXT } else { c::TEXT_DIM };
            let icon = Rect::from_center_size(
                Pos2::new(tab_rect.left() + 12.0, tab_rect.center().y),
                Vec2::splat(sp::ICON),
            );
            match tab.kind {
                TabKind::Folder => Icon::Folder.paint(painter, icon, text_color),
                TabKind::File if tab.doc.kind == DocKind::Markdown => {
                    Icon::Markdown.paint(painter, icon, text_color)
                }
                TabKind::File => Icon::File.paint(painter, icon, text_color),
            }

            let text_x = icon.right() + 6.0;
            let name_w = (tab_rect.right() - text_x - close_w - 6.0).max(20.0);
            let name_g = widgets::layout_elided(
                ui,
                tab.label(),
                theme::ui_font(tfs::SMALL),
                text_color,
                name_w,
            );
            widgets::galley_at(
                painter,
                Pos2::new(text_x, tab_rect.center().y - name_g.size().y * 0.5),
                &name_g,
                text_color,
            );

            // Unsaved changes: a dot, the same signal as the header uses.
            if dirty {
                painter.circle_filled(
                    Pos2::new(text_x + name_g.size().x + 5.0, tab_rect.center().y),
                    2.5,
                    c::ACCENT,
                );
            }

            // The close cross, which only lights up under the pointer.
            let close_rect = Rect::from_center_size(
                Pos2::new(tab_rect.right() - 11.0, tab_rect.center().y),
                Vec2::splat(close_w),
            );
            let close_resp = ui.interact(close_rect, Id::new(("tab-close", i)), Sense::click());
            if close_resp.clicked() {
                close = Some(i);
            }
            if close_resp.hovered() || resp.hovered() {
                Icon::Close.paint(painter, close_rect, c::TEXT_DIM);
            }
            let _ = close_resp.on_hover_text(if dirty {
                "Close without saving"
            } else {
                "Close (Ctrl+W)"
            });
        }

        if let Some(i) = focus {
            self.focus_tab(i);
        }
        if let Some(i) = close
            && let Some(tab) = self.tabs.get(i)
        {
            let path = tab.path().to_path_buf();
            if tab.doc.dirty() {
                self.dialog = Dialog::Unsaved {
                    path,
                    close_app: false,
                };
                // Remember which tab the prompt is about.
                self.pending_close = Some(i);
            } else {
                self.close_tab(i);
            }
        }
    }

    fn filter_focused(&self, ctx: &Context) -> bool {
        ctx.memory(|m| m.has_focus(Id::new(ID_SEARCH)))
    }

    // ---- editor text transforms --------------------------------------------

    /// Runs after a single character was typed: auto-indent, auto-close,
    /// skip-over.
    fn auto_format(&mut self, ctx: &Context, path: &Path, id: Id, range: Option<CCursorRange>) {
        let _ = path;
        let Some(range) = range else { return };
        let primary = range.primary.index.0;
        if primary == 0 {
            return;
        }
        let Some(doc) = self.doc_mut() else { return };
        let typed = editing::char_at(&doc.text, primary - 1);
        let Some(typed) = typed else { return };

        if typed == '\n' {
            // Keep the indentation of the line we just split.
            let caret = editing::auto_indent(&mut doc.text, primary);
            self.after_edit(ctx, id, Some(caret));
            return;
        }
        if editing::is_opener(typed)
            && let Some(next) = editing::auto_close(&mut doc.text, primary)
        {
            self.after_edit(ctx, id, Some(next));
        }
    }

    fn indent_lines(&mut self, ctx: &Context, id: Id, range: Option<CCursorRange>, outdent: bool) {
        let Some(range) = range else { return };
        let (lo, hi) = ordered(range.primary.index.0, range.secondary.index.0);
        let Some(doc) = self.doc_mut() else { return };
        let caret = editing::indent(&mut doc.text, lo..hi, outdent);
        self.after_edit(ctx, id, caret);
    }

    fn toggle_comment(&mut self, ctx: &Context, path: &Path, id: Id, range: Option<CCursorRange>) {
        let Some(range) = range else { return };
        let (lo, hi) = ordered(range.primary.index.0, range.secondary.index.0);
        let token = editing::comment_token(path);
        let Some(doc) = self.doc_mut() else { return };
        let caret = editing::toggle_comment(&mut doc.text, lo..hi, token);
        self.after_edit(ctx, id, caret);
    }

    /// Marks the buffer edited, schedules a Markdown refresh, and moves the
    /// caret when our own edit invalidated it.
    fn after_edit(&mut self, ctx: &Context, id: Id, caret: Option<usize>) {
        if let Some(doc) = self.doc_mut() {
            doc.touch();
            self.render_version = doc.version;
        }
        self.last_edit = Some(Instant::now());
        if let Some(caret) = caret {
            self.set_text_cursor(ctx, id, caret);
        }
    }

    /// Moves the text widget's caret, so our own edits do not fight it.
    fn set_text_cursor(&self, ctx: &Context, id: Id, caret: usize) {
        use egui::widgets::text_edit::TextEditState;
        if let Some(mut state) = TextEditState::load(ctx, id) {
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
            state.store(ctx, id);
        }
    }

    // ---- messages -----------------------------------------------------------

    /// Applies everything the workers have finished. Nothing here touches the
    /// disk, so a busy copy or search never stalls a frame.
    fn drain_messages(&mut self, ctx: &Context) {
        for msg in workers::drain(&self.rx) {
            match msg {
                Msg::Listed {
                    token,
                    path,
                    entries,
                    error,
                } => {
                    if token != self.req || path != self.cwd {
                        continue;
                    }
                    match error {
                        Some(e) => {
                            log::warn!("listing failed: {e}");
                            self.entries.clear();
                            self.visible.clear();
                            self.listing = Listing::Failed;
                        }
                        None => {
                            self.entries = entries;
                            self.listed_at = Some(Instant::now());
                            fs_model::sort(&mut self.entries, self.sort, self.ascending);
                            self.listing = Listing::Ready;
                            self.row_cache.clear();
                            self.thumbs.clear();
                            self.recompute_visible();
                        }
                    }
                    let cwd = self.cwd.clone();
                    self.watch(&cwd);
                }
                Msg::Loaded {
                    path, text, error, ..
                } => {
                    if self.loading.as_deref() != Some(path.as_path()) {
                        continue;
                    }
                    self.loading = None;
                    match error {
                        Some(e) => self.toast_err(e),
                        None => {
                            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                            let doc = Doc::from_parts(
                                path.clone(),
                                text,
                                editor::doc_kind(&path),
                                mtime,
                                false,
                            );
                            // The tab was created when the read started, so the
                            // text goes into the tab that is already on screen.
                            match self.tab_index(&path) {
                                Some(i) => {
                                    self.tabs.focus(i);
                                    if let Some(t) = self.tabs.get_mut(i) {
                                        t.kind = TabKind::File;
                                        t.doc = doc;
                                    }
                                }
                                None => {
                                    let i = self.tabs.push(Tab {
                                        kind: TabKind::File,
                                        doc,
                                    });
                                    self.tabs.active = i;
                                }
                            }
                            self.preview.reset();
                            self.render_version = 1;
                            self.preview.reset();
                        }
                    }
                }
                Msg::Progress(p) => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.job.id == p.id) {
                        j.done_items = p.done_items;
                        j.total_items = p.total_items;
                        j.done_bytes = p.done_bytes;
                        j.total_bytes = p.total_bytes;
                        j.current = p.current;
                        if !p.failed.is_empty() {
                            self.toast_err(p.failed[0].clone());
                        }
                    }
                }
                Msg::Finished { id, outcome } => {
                    self.jobs.retain(|j| j.job.id != id);
                    self.settle_pending_undo();
                    // The job changed what is on disk, so cached subtree
                    // totals for the folders it touched are no longer true.
                    self.measures.clear();
                    if let Outcome::Failed(msg) = &outcome {
                        self.toast_err(msg.clone());
                    }
                    self.request_listing();
                    match outcome {
                        Outcome::Done { ok, failed } => {
                            if failed.is_empty() {
                                self.toast(format!("Done \u{00B7} {ok} item(s)"));
                            } else {
                                self.toast_err(format!("{ok} done, {} failed", failed.len()));
                                for f in failed.iter().take(3) {
                                    log::warn!("op failed: {f}");
                                }
                            }
                        }
                        Outcome::Cancelled { done } => {
                            self.toast(format!("Cancelled after {done} item(s)"));
                        }
                        Outcome::Failed(e) => self.toast_err(e),
                    }
                }
                Msg::Search(chunk) => {
                    self.search.results.extend(chunk.found);
                    self.search.scanned = chunk.scanned;
                    self.search.truncated = chunk.truncated;
                    if chunk.done {
                        self.search.running = false;
                        self.search_shown = true;
                        self.row_cache.clear();
                    }
                }
                Msg::Peek { path, text } => {
                    if self.peek_pending.as_deref() == Some(path.as_path()) {
                        self.peek_pending = None;
                    }
                    if let Some(text) = text {
                        self.peek = Some((path, text));
                    }
                }
                Msg::Thumb {
                    path,
                    px,
                    rgba,
                    w,
                    h,
                } => {
                    self.thumbs.insert(path, px, rgba, w, h, ctx);
                }
                Msg::Measured { path, measure } => {
                    self.measures.set(path, measure);
                }
                Msg::TreeLoaded { path, dirs } => {
                    if self.sidebar_tree.loading.as_deref() == Some(path.as_path()) {
                        self.sidebar_tree.loading = None;
                    }
                    self.sidebar_tree.tree.set_children(&path, dirs);
                }
                Msg::Watch(path) => {
                    self.last_change = Some(Instant::now());
                    if let Some(doc) = self.doc_mut()
                        && doc.path == path
                    {
                        doc.check_external_change();
                    }
                }
            }
        }
    }

    fn handle_watch_debounce(&mut self) {
        let Some(last) = self.last_change else { return };
        if last.elapsed() < WATCH_DEBOUNCE {
            return;
        }
        self.last_change = None;
        // Reading a folder can itself raise a change event, so a refresh can
        // trigger the next one. Ignoring events for a moment after each listing
        // breaks that loop; a real edit still shows up, just not instantly.
        if let Some(listed) = self.listed_at
            && listed.elapsed() < WATCH_COOLDOWN
        {
            return;
        }
        self.request_listing();
        if let Some(doc) = self.doc() {
            let dir = doc.path.parent().map(|p| p.to_path_buf());
            if let Some(dir) = dir {
                self.watch(&dir);
            }
        }
    }

    /// Reacts to a change in the search box: filters the current folder at once,
    /// then starts a recursive search after a short pause.
    fn on_filter_changed(&mut self) {
        self.search_typed = Some(Instant::now());
        self.row_cache.clear();
        if self.filter.trim().is_empty() {
            self.clear_search();
            self.recompute_visible();
            return;
        }
        self.recompute_visible();
        if self.scope == SearchScope::Below {
            self.pump_search();
        }
    }

    /// Starts (or refreshes) the recursive search once typing pauses.
    fn pump_search(&mut self) {
        if self.scope != SearchScope::Below || self.filter.trim().is_empty() {
            return;
        }
        if let Some(typed) = self.search_typed
            && typed.elapsed() < SEARCH_DEBOUNCE
        {
            return;
        }
        self.search_typed = None;
        self.search.start(&self.cwd, &self.filter, self.tx.clone());
        self.search_shown = true;
        self.row_cache.clear();
    }

    // ---- navigation ---------------------------------------------------------

    fn navigate(&mut self, path: &Path) {
        let path = fs_model::normalize(path);
        if path == self.cwd {
            return;
        }
        self.history.push(self.cwd.clone());
        self.cwd = path.clone();
        // Show where we landed in the tree.
        self.sidebar_tree.tree.reveal(&path);
        self.after_jump();
    }

    fn go_back(&mut self) {
        if let Some(prev) = self.history.back(&self.cwd) {
            self.cwd = fs_model::normalize(&prev);
            self.after_jump();
        }
    }

    fn go_forward(&mut self) {
        if let Some(next) = self.history.forward(&self.cwd) {
            self.cwd = fs_model::normalize(&next);
            self.after_jump();
        }
    }

    /// Shared reset after the folder changes.
    fn after_jump(&mut self) {
        self.sel.clear();
        self.typeahead.clear();
        self.cursor = 0;
        self.anchor = 0;
        self.filter.clear();
        self.search_typed = None;
        self.clear_search();
        self.row_cache.clear();
        self.listing = Listing::Loading;
        self.request_listing();
    }

    fn go_up(&mut self) {
        if self.searching() {
            self.clear_search();
            self.recompute_visible();
            return;
        }
        let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        if parent == self.cwd {
            return;
        }
        let came_from = self
            .cwd
            .file_name()
            .map(|s| s.to_string_lossy().to_string());
        self.navigate(&parent);
        if let Some(name) = came_from
            && let Some(i) = self
                .visible
                .iter()
                .position(|e| self.entries.get(*e).is_some_and(|e| e.name == name))
        {
            self.cursor = i;
            self.anchor = i;
            if let Some(e) = self.entries.get(self.visible[i]) {
                self.sel.insert(e.path.clone());
            }
        }
    }

    /// Re-reads the current folder on a worker thread.
    fn request_listing(&mut self) {
        self.req = self.ids.next();
        let token = self.req;
        let path = self.cwd.clone();
        let show_hidden = self.show_hidden;
        let tx = self.tx.clone();
        self.listing = Listing::Loading;
        self.row_cache.clear();
        let _ = std::thread::Builder::new()
            .name("xplor-list".into())
            .spawn(move || {
                let (entries, error) = match fs_model::read_dir(&path, show_hidden) {
                    Ok(e) => (e, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                let _ = tx.send(Msg::Listed {
                    token,
                    path,
                    entries,
                    error,
                });
            });
    }

    /// Rebuilds the visible index list from the filter.
    fn recompute_visible(&mut self) {
        let filter = self.filter.trim();
        self.visible.clear();
        for (i, e) in self.entries.iter().enumerate() {
            if filter.is_empty() || search::matches(&e.name, filter) {
                self.visible.push(i);
            }
        }
        let count = self.visible.len();
        if self.cursor >= count {
            self.cursor = count.saturating_sub(1);
        }
        self.anchor = self.cursor;
        self.row_cache.clear();
    }

    fn apply_sort(&mut self) {
        fs_model::sort(&mut self.entries, self.sort, self.ascending);
        self.recompute_visible();
    }

    fn row_count(&self) -> usize {
        if self.searching() {
            self.search.results.len()
        } else {
            self.visible.len()
        }
    }

    fn searching(&self) -> bool {
        self.search.running || self.search_shown
    }

    fn clear_search(&mut self) {
        self.search.cancel();
        self.search.results.clear();
        self.search.scanned = 0;
        self.search_shown = false;
        self.search_typed = None;
        self.row_cache.clear();
    }

    fn cursor_path(&self) -> Option<PathBuf> {
        self.path_at(self.cursor)
    }

    fn path_at(&self, i: usize) -> Option<PathBuf> {
        if self.searching() {
            self.search.results.get(i).map(|e| e.path.clone())
        } else {
            self.visible
                .get(i)
                .and_then(|e| self.entries.get(*e))
                .map(|e| e.path.clone())
        }
    }

    /// The selected paths, or the row under the cursor when nothing is selected.
    fn target_paths(&self) -> Vec<PathBuf> {
        if self.sel.is_empty() {
            self.cursor_path().into_iter().collect()
        } else {
            self.sel.iter().cloned().collect()
        }
    }

    fn select_all(&mut self) {
        self.sel.clear();
        for i in 0..self.row_count() {
            if let Some(p) = self.path_at(i) {
                self.sel.insert(p);
            }
        }
    }

    // ---- opening files ---------------------------------------------------------

    /// Opens a file in a tab, or focuses the tab that already has it.
    fn open_path(&mut self, path: &Path) {
        if path.is_dir() {
            self.navigate(path);
            return;
        }
        // Already open: just bring it forward, the way Explorer does.
        if let Some(i) = self.tab_index(path) {
            self.focus_tab(i);
            return;
        }
        // Not something we can read: hand it to the desktop and add no tab.
        if !fs_model::is_editable_text(path) {
            if let Err(e) = editor::open_externally(path) {
                self.toast_err(e);
            }
            return;
        }

        // A folder tab becomes the document, so one tab never shows two things.
        let on_folder_tab = self
            .tabs
            .active_tab()
            .is_some_and(|t| t.kind == TabKind::Folder);
        if on_folder_tab {
            // Reuse the slot, so the tab keeps its place in the strip.
            if let Some(t) = self.tabs.active_tab_mut() {
                t.kind = TabKind::File;
                t.doc = Doc::placeholder(path);
            }
        } else {
            let i = self.tabs.push(Tab::file(path));
            self.tabs.active = i;
        }
        self.preview.reset();
        self.render_version = 0;
        self.last_edit = None;

        // Read on a worker thread so a large file never stalls the UI. The tab
        // exists already, so the strip shows it while the text arrives.
        self.loading = Some(path.to_path_buf());
        let tx = self.tx.clone();
        let read_path = path.to_path_buf();
        let watch_dir = path.parent().map(|p| p.to_path_buf());
        let _ = std::thread::Builder::new()
            .name("xplor-read".into())
            .spawn(move || {
                let result = Doc::read(&read_path);
                let _ = tx.send(Msg::Loaded {
                    path: result.0,
                    text: result.1,
                    error: result.2,
                });
            });
        if let Some(dir) = watch_dir {
            self.watch(&dir);
        }
    }

    /// Opens a new tab showing the file list, the way Explorer does.
    ///
    /// A tab is either a document or the folder view. A folder tab remembers
    /// the folder it was opened at, so focusing it goes back there.
    fn new_tab(&mut self) {
        if self
            .tabs
            .active_tab()
            .is_some_and(|t| matches!(t.kind, TabKind::Folder))
        {
            self.toast("This tab is already showing files".into());
            return;
        }
        let dir = self.cwd.clone();
        let i = self.tabs.push(Tab::folder(&dir));
        self.tabs.active = i;
        self.preview.reset();
    }

    fn close_doc(&mut self) {
        if !self.has_tabs() {
            return;
        }
        // Ctrl+W closes the active tab, prompting if that one is dirty.
        let active = self.tabs.active;
        if self.tabs[active].doc.dirty() {
            self.pending_close = Some(active);
            let path = self.tabs[active].doc.path.clone();
            self.dialog = Dialog::Unsaved {
                path,
                close_app: false,
            };
            return;
        }
        self.close_tab(active);
    }

    fn save_doc(&mut self) {
        let Some(doc) = self.doc_mut() else { return };
        if !doc.dirty() {
            return;
        }
        let name = doc.file_name();
        match doc.save() {
            Ok(()) => {
                self.render_version = doc.version;
                self.toast(format!("Saved {name}"));
            }
            Err(e) => self.toast_err(e),
        }
    }

    // ---- file operations --------------------------------------------------------

    fn copy_selection(&mut self, cut: bool) {
        let mut paths = self.target_paths();
        if paths.is_empty() {
            return;
        }
        paths.sort();
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let text = paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = cb.set_text(text);
        }
        let n = paths.len();
        self.clip = Some(Clipboard { paths, cut });
        self.toast(format!(
            "{} {n} item(s)",
            if cut { "Cut" } else { "Copied" }
        ));
    }

    fn paste(&mut self) {
        let sources: Vec<PathBuf> = match self.clip.clone() {
            Some(clip) if clip.paths.iter().any(|p| p.exists()) => clip.paths,
            _ => self.system_paths(),
        };
        if sources.is_empty() {
            self.toast("Clipboard holds no files".into());
            return;
        }
        let cut = self.clip.as_ref().is_some_and(|c| c.cut);
        self.start_transfer(sources, self.cwd.clone(), cut);
        if cut {
            self.clip = None;
        }
    }

    /// Kicks off a copy or move, recording it for undo.
    fn start_transfer(&mut self, sources: Vec<PathBuf>, dest: PathBuf, cut: bool) {
        if sources.is_empty() {
            return;
        }
        // Work out where everything lands so undo knows the reverse mapping.
        let mut pairs = Vec::with_capacity(sources.len());
        for src in &sources {
            let Some(name) = src.file_name() else {
                continue;
            };
            // Never overwrite: a colliding name gets " (2)" and friends.
            let target = dest.join(name);
            let target = if target == *src || target.exists() {
                fs_model::unique_dest(&target)
            } else {
                target
            };
            pairs.push((src.clone(), target));
        }
        if pairs.is_empty() {
            return;
        }
        let id = self.ids.next();
        let from: Vec<PathBuf> = pairs.iter().map(|(s, _)| s.clone()).collect();
        let to: Vec<PathBuf> = pairs.iter().map(|(_, t)| t.clone()).collect();
        let job = ops::start_transfer_pairs(self.tx.clone(), id, pairs.clone(), cut);
        let _ = from;
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(if cut {
            Undo::Moved { items: pairs }
        } else {
            Undo::Copied { items: to }
        });
    }

    /// Moves items that were dropped onto a folder.
    fn drop_onto(&mut self, paths: Vec<PathBuf>, dest: &Path, copy: bool) {
        let mut paths = paths;
        paths.retain(|p| p.parent() != Some(dest));
        if paths.is_empty() {
            return;
        }
        self.start_transfer(paths, dest.to_path_buf(), !copy);
    }

    /// Puts one path on the clipboard as text, which is what a shell expects.
    fn copy_as_path(&mut self, path: &Path) {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            self.toast_err("No clipboard available".into());
            return;
        };
        if cb.set_text(path.to_string_lossy().into_owned()).is_err() {
            self.toast_err("Could not write to the clipboard".into());
        }
    }

    /// Opens a terminal in the folder that holds `path`.
    ///
    /// Each platform gets the terminal it actually ships with, because a wrong
    /// guess would silently do nothing.
    fn open_in_terminal(&mut self, path: &Path) {
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        };
        let candidates: &[(&str, &[&str])] = if cfg!(windows) {
            &[
                ("wt.exe", &["-d", "."]),
                ("powershell.exe", &["-NoExit", "-Command", "Set-Location ."]),
                ("cmd.exe", &["/k", "cd ."]),
            ]
        } else if cfg!(target_os = "macos") {
            &[("open", &["-a", "Terminal", "."])]
        } else {
            &[
                ("x-terminal-emulator", &["./"]),
                ("gnome-terminal", &["./"]),
                ("konsole", &["./"]),
                ("xterm", &["./"]),
            ]
        };
        let mut launched = false;
        for (program, args) in candidates {
            let r = std::process::Command::new(program)
                .args(*args)
                .current_dir(&dir)
                .spawn();
            if r.is_ok() {
                launched = true;
                break;
            }
        }
        if !launched {
            self.toast_err("No terminal found on this system".into());
        }
    }

    /// Compresses the selection into a zip beside it.
    fn start_zip(&mut self) {
        let sources: Vec<PathBuf> = self.sel.iter().cloned().collect();
        if sources.is_empty() {
            return;
        }
        // The archive lands next to the first item, like Explorer does.
        let dest_dir = sources[0]
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.cwd.clone());
        let id = self.ids.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ops::start_zip(self.tx.clone(), id, sources.clone(), dest_dir, cancel);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(Undo::Created(Vec::new()));
    }

    fn system_paths(&self) -> Vec<PathBuf> {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return Vec::new();
        };
        let Ok(text) = cb.get_text() else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| {
                let p = PathBuf::from(l.trim());
                p.exists().then_some(p)
            })
            .collect()
    }

    fn delete_selection(&mut self, permanent: bool) {
        let paths = self.target_paths();
        if paths.is_empty() {
            return;
        }
        if permanent {
            self.dialog = Dialog::ConfirmDelete { paths };
            return;
        }
        // Default: the OS recycle bin, so nothing is lost by accident.
        match ops::send_to_trash(&paths) {
            Ok(()) => {
                self.toast(format!("Moved {} item(s) to trash", paths.len()));
                self.sel.clear();
                self.request_listing();
            }
            Err(e) => self.toast_err(e),
        }
    }

    fn start_permanent_delete(&mut self, paths: Vec<PathBuf>) {
        let id = self.ids.next();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ops::start_permanent_delete(self.tx.clone(), id, paths.clone(), cancel);
        self.jobs.push(ActiveJob {
            done_items: 0,
            total_items: 0,
            done_bytes: 0,
            total_bytes: 0,
            current: String::new(),
            job,
        });
        self.pending_undo = Some(Undo::Created(paths));
        self.sel.clear();
    }

    fn start_rename(&mut self, path: &Path) {
        self.dialog = Dialog::Rename {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        };
    }

    fn apply_rename(&mut self, path: &Path, name: &str) {
        let name = name.trim();
        let Some(parent) = path.parent() else { return };
        let target = parent.join(name);
        if target == path {
            return;
        }
        if target.exists() {
            self.toast_err(format!("{name} already exists"));
            return;
        }
        match std::fs::rename(path, &target) {
            Ok(()) => {
                self.undo = Some(Undo::Renamed {
                    from: path.to_path_buf(),
                    to: target.clone(),
                });
                self.undo_stack.push(self.undo.clone().expect("just set"));
                self.toast(format!("Renamed to {name}"));
                if let Some(doc) = self.doc_mut()
                    && doc.path == path
                {
                    doc.path = target;
                }
                self.request_listing();
            }
            Err(e) => self.toast_err(format!("Rename failed: {e}")),
        }
    }

    fn apply_create(&mut self, dir: &Path, name: &str, folder: bool) {
        let name = name.trim();
        let target = dir.join(name);
        if target.exists() {
            self.toast_err(format!("{name} already exists"));
            return;
        }
        let r = if folder {
            std::fs::create_dir(&target)
        } else {
            std::fs::write(&target, b"")
        };
        match r {
            Ok(()) => {
                self.undo = Some(Undo::Created(vec![target.clone()]));
                self.undo_stack.push(self.undo.clone().expect("just set"));
                self.request_listing();
                if folder {
                    self.toast(format!("Created folder {name}"));
                } else {
                    self.toast(format!("Created {name}"));
                    self.open_path(&target);
                }
            }
            Err(e) => self.toast_err(format!("Could not create {name}: {e}")),
        }
    }

    /// Rolls back the last operation, the way Explorer does with Ctrl+Z.
    fn undo(&mut self) {
        let Some(action) = self.undo.take() else {
            self.toast("Nothing to undo".into());
            return;
        };
        let mut problems: Vec<String> = Vec::new();
        match action {
            Undo::Created(paths) => {
                for p in &paths {
                    let r = if p.is_dir() {
                        std::fs::remove_dir_all(p)
                    } else {
                        std::fs::remove_file(p)
                    };
                    if let Err(e) = r {
                        problems.push(format!("{}: {e}", display_name(p)));
                    }
                }
                self.toast("Undo: removed the new item(s)".into());
            }
            Undo::Renamed { from, to } => {
                if let Err(e) = std::fs::rename(&to, &from) {
                    problems.push(format!("{}: {e}", display_name(&to)));
                } else if let Some(doc) = self.doc_mut()
                    && doc.path == to
                {
                    doc.path = from;
                }
                self.toast("Undo: rename reverted".into());
            }
            Undo::Moved { items } => {
                for (src, dst) in items {
                    // Move the destination back to where it came from.
                    let r = if std::fs::rename(&dst, &src).is_ok() {
                        Ok(())
                    } else {
                        ops::move_now(&dst, &src)
                    };
                    if let Err(e) = r {
                        problems.push(format!("{}: {e}", display_name(&dst)));
                    }
                }
                self.toast("Undo: move reverted".into());
            }
            Undo::Copied { items } => {
                for p in &items {
                    let r = if p.is_dir() {
                        std::fs::remove_dir_all(p)
                    } else {
                        std::fs::remove_file(p)
                    };
                    if let Err(e) = r {
                        problems.push(format!("{}: {e}", display_name(p)));
                    }
                }
                self.toast("Undo: copies removed".into());
            }
        }
        self.request_listing();
        if problems.is_empty() {
            if let Some(next) = self.undo_stack.pop() {
                self.undo = Some(next);
            }
        } else {
            self.toast_err(format!("Undo incomplete: {}", problems.join("; ")));
        }
    }

    /// Records the operation a worker just finished, if it is undoable.
    fn settle_pending_undo(&mut self) {
        if let Some(action) = self.pending_undo.take() {
            self.undo_stack.push(action);
            self.undo = self.undo_stack.last().cloned();
        }
    }

    /// Handles files dragged in from the OS, plus our own drag-and-drop
    /// payloads (paths encoded as a text list, the same format Explorer uses).
    fn handle_file_drop(&mut self, ctx: &Context) {
        let mut dropped: Vec<PathBuf> = Vec::new();
        ctx.input(|i| {
            for f in &i.raw.dropped_files {
                dropped.push(f.path().to_path_buf());
            }
        });

        // Our own payload: newline separated paths, matching Explorer.
        if dropped.is_empty() {
            let mut text = String::new();
            ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Paste(t) = ev {
                        text.push_str(t);
                    }
                }
            });
            if !text.is_empty() {
                dropped = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .collect();
            }
        }
        if dropped.is_empty() {
            return;
        }

        // Dropped on a folder row or the sidebar: move into it. Dropped on
        // empty space: copy into the current folder.
        let target = self.drop_target.take();
        match target {
            Some(folder) if folder.is_dir() => {
                let copy = ctx.input(|i| i.modifiers.ctrl || i.modifiers.command);
                self.drop_onto(dropped, &folder, copy);
            }
            _ => {
                for path in dropped {
                    if path.is_dir() {
                        self.navigate(&path);
                    } else {
                        if let Some(parent) = path.parent() {
                            self.navigate(parent);
                        }
                        self.open_path(&path);
                    }
                }
            }
        }
    }

    // ---- watching ---------------------------------------------------------------

    /// Watches one folder for changes, so external edits show up on their own.
    fn watch(&mut self, path: &Path) {
        if self.watch_target.as_deref() == Some(path) {
            return;
        }
        self.watcher = None;
        self.watch_target = None;
        let tx = self.tx.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res
                    && let Some(p) = ev.paths.first()
                {
                    let _ = tx.send(Msg::Watch(p.clone()));
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("watcher unavailable: {e}");
                    return;
                }
            };
        if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
            log::warn!("cannot watch {}: {e}", path.display());
            return;
        }
        self.watcher = Some(Box::new(watcher));
        self.watch_target = Some(path.to_path_buf());
    }

    // ---- toasts -------------------------------------------------------------------

    fn toast(&mut self, text: String) {
        self.toasts.push(Toast {
            text,
            danger: false,
            born: Instant::now(),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    fn toast_err(&mut self, text: String) {
        log::warn!("{text}");
        self.toasts.push(Toast {
            text,
            danger: true,
            born: Instant::now(),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    fn expire_toasts(&mut self) {
        let now = Instant::now();
        self.toasts
            .retain(|t| now.duration_since(t.born) < TOAST_TTL);
    }
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
            (true, None) => String::from("\u{2014}"),
            (false, _) => md
                .as_ref()
                .map_or_else(|| String::from("\u{2014}"), |m| fs_model::fmt_size(m.len())),
        },
    );
    if let Some(m) = md.as_ref() {
        if is_dir {
            row(
                ui,
                "Contains",
                measures.map_or_else(
                    || String::from("\u{2014}"),
                    |m| format!("{} files, {} folders", m.files, m.folders),
                ),
            );
        }
        row(
            ui,
            "Created",
            m.created().map_or(String::from("\u{2014}"), fmt_stamp),
        );
        row(
            ui,
            "Modified",
            m.modified().map_or(String::from("\u{2014}"), fmt_stamp),
        );
        row(
            ui,
            "Accessed",
            m.accessed().map_or(String::from("\u{2014}"), fmt_stamp),
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
            "\u{2014}".to_owned()
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
        Err(_) => String::from("\u{2014}"),
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

fn ordered(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
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

/// Monochrome editor highlighting profile: every token is a shade of grey.
fn editor_theme() -> egui_code_editor::ColorTheme {
    egui_code_editor::ColorTheme {
        name: "xplor",
        dark: true,
        bg: "0D0E10",
        cursor: "8C9199",
        selection: "2C2F35",
        comments: "6A6E76",
        functions: "E4E6EA",
        keywords: "F6F7F8",
        literals: "B4B8C0",
        numerics: "B4B8C0",
        punctuation: "868B94",
        strs: "9DA2AB",
        types: "D8DAE0",
        special: "FFFFFF",
    }
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
        .inner_margin(Margin::symmetric(sp::SM_I, 0))
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

/// Persisted preferences live in a plain text file next to the log.
fn prefs_path() -> PathBuf {
    dirs::config_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("xplor")
        .join("prefs.txt")
}

fn sort_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Size => "size",
        SortKey::Modified => "modified",
        SortKey::Ext => "ext",
    }
}

impl eframe::App for Xplor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.draw(ui, &ctx);
    }

    fn on_exit(&mut self) {
        self.write_prefs();
        log::info!("xplor exiting");
    }
}

use egui::text::CCursor;
use egui::text_selection::CCursorRange;

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
mod tests {
    use super::*;

    fn file(name: &str) -> Tab {
        Tab::file(Path::new(name))
    }

    #[test]
    fn pins_keep_the_order_they_were_added_in() {
        let mut p = Pins::default();
        p.add(Path::new("/work/app"), 8).unwrap();
        p.add(Path::new("/work/notes"), 8).unwrap();
        p.add(Path::new("/home"), 8).unwrap();
        assert_eq!(
            *p,
            vec![
                PathBuf::from("/work/app"),
                PathBuf::from("/work/notes"),
                PathBuf::from("/home")
            ]
        );
    }

    #[test]
    fn pinning_the_same_folder_twice_does_nothing() {
        let mut p = Pins::default();
        p.add(Path::new("/work"), 8).unwrap();
        // Same folder, spelled with a trailing separator and a detour.
        let again = p.add(Path::new("/work/./"), 8);
        assert!(matches!(again, Err(PinError::AlreadyThere)));
        assert_eq!(p.len(), 1);
        assert!(p.contains(Path::new("/work")));
    }

    #[test]
    fn pins_stop_at_the_cap() {
        let mut p = Pins::default();
        p.add(Path::new("/a"), 2).unwrap();
        p.add(Path::new("/b"), 2).unwrap();
        assert!(matches!(p.add(Path::new("/c"), 2), Err(PinError::Full)));
        assert_eq!(p.len(), 2, "the rejected folder was not added");
        // Freeing a slot lets the next one in.
        assert!(p.remove(Path::new("/a")));
        p.add(Path::new("/c"), 2).unwrap();
        assert_eq!(*p, vec![PathBuf::from("/b"), PathBuf::from("/c")]);
    }

    #[test]
    fn unpinning_says_whether_the_folder_was_there() {
        let mut p = Pins::default();
        p.add(Path::new("/a"), 8).unwrap();
        p.add(Path::new("/b"), 8).unwrap();
        assert!(p.remove(Path::new("/a")));
        assert!(!p.remove(Path::new("/a")), "removed twice");
        assert!(!p.remove(Path::new("/nowhere")));
        assert_eq!(*p, vec![PathBuf::from("/b")]);
    }

    fn folder(name: &str) -> Tab {
        Tab::folder(Path::new(name))
    }

    fn three_files() -> Tabs {
        let mut t = Tabs::default();
        t.push(file("/a/one.txt"));
        t.push(file("/a/two.txt"));
        t.push(file("/a/three.txt"));
        t
    }

    #[test]
    fn closing_a_middle_tab_keeps_the_same_file_on_screen() {
        let mut t = three_files();
        t.active = 2;
        // Closing the tab before the focus pulls the index back, so the file
        // that was on screen stays on screen.
        t.close(0);
        assert_eq!(t.active, 1);
        assert_eq!(t.active_tab().map(Tab::label).as_deref(), Some("three.txt"));
    }

    #[test]
    fn closing_the_tab_on_screen_falls_back_to_the_new_last() {
        let mut t = three_files();
        t.active = 2;
        t.close(2);
        assert_eq!(t.active, 1);
        assert_eq!(t.active_tab().map(Tab::label).as_deref(), Some("two.txt"));
    }

    #[test]
    fn closing_the_last_tab_empties_the_strip_safely() {
        let mut t = Tabs::default();
        t.push(file("/a/only.txt"));
        t.close(0);
        assert!(t.is_empty());
        assert_eq!(t.active, 0, "the index stays usable for the next open");
        // Closing again must not panic.
        t.close(0);
        t.close(5);
        assert!(t.is_empty());
    }

    #[test]
    fn cycling_wraps_at_both_ends() {
        let mut t = three_files();
        t.active = 0;
        t.cycle(false);
        assert_eq!(t.active, 1);
        t.cycle(false);
        t.cycle(false);
        assert_eq!(t.active, 0, "forward wraps to the first");
        t.cycle(true);
        assert_eq!(t.active, 2, "backwards wraps to the last");
    }

    #[test]
    fn a_single_tab_never_cycles() {
        let mut t = Tabs::default();
        t.push(file("/a/only.txt"));
        t.cycle(false);
        t.cycle(true);
        assert_eq!(t.active, 0);
    }

    #[test]
    fn an_empty_strip_never_cycles() {
        let mut t = Tabs::default();
        t.cycle(false);
        t.cycle(true);
        assert!(t.active_tab().is_none());
    }

    #[test]
    fn focusing_ignores_an_index_past_the_end() {
        let mut t = three_files();
        t.focus(1);
        assert_eq!(t.active, 1);
        t.focus(9);
        assert_eq!(t.active, 1, "an out-of-range focus is ignored");
    }

    #[test]
    fn only_a_file_tab_shows_the_editor() {
        let mut t = Tabs::default();
        t.push(folder("/a/photos"));
        assert!(!t.shows_file(), "a folder tab shows the file list");
        t.push(file("/a/notes.md"));
        t.active = 1;
        assert!(t.shows_file());
    }

    #[test]
    fn tabs_are_named_after_their_file_or_folder() {
        assert_eq!(file("/a/notes.md").label(), "notes.md");
        assert_eq!(folder("/a/photos").label(), "photos");
        // A drive root has no name of its own.
        assert_eq!(folder("/").label(), "Files");
    }

    #[test]
    fn an_open_file_is_found_by_path() {
        let mut t = three_files();
        assert_eq!(t.index_of(Path::new("/a/two.txt")), Some(1));
        assert_eq!(t.index_of(Path::new("/a/missing.txt")), None);
        t.close(1);
        assert_eq!(t.index_of(Path::new("/a/two.txt")), None);
    }

    #[test]
    fn only_dirty_tabs_are_reported() {
        let mut t = three_files();
        assert_eq!(t.first_dirty(), None);
        t[1].doc.text.push_str("edited");
        t[1].doc.version += 1;
        assert_eq!(t.first_dirty(), Some(1));
    }
}

#[cfg(test)]
mod col_tests {
    use super::*;

    const WIDE: f32 = 900.0;
    const NARROW: f32 = 330.0;

    #[test]
    fn a_wide_pane_keeps_both_columns_at_their_dragged_width() {
        assert_eq!(fit_columns(WIDE, 92.0, 132.0), (92.0, 132.0));
    }

    #[test]
    fn a_narrow_pane_keeps_the_name_readable() {
        let (size, date) = fit_columns(NARROW, 92.0, 132.0);
        // Whatever happens, the name keeps its minimum, so the columns must
        // have given way rather than the name being squeezed to nothing.
        let chrome = sp::SM + sp::ICON + sp::SM + sp::MD + sp::MD + sp::SM;
        let name_room = NARROW - chrome - size - date;
        assert!(
            name_room >= MIN_NAME_COL,
            "name left only {name_room}px with columns {size}/{date}"
        );
    }

    #[test]
    fn the_date_column_is_dropped_before_the_size_one() {
        // Ask for far more than any pane holds, then narrow the pane until only
        // one column can stay: the size survives, because a size is more useful
        // at a glance than a timestamp.
        let (size, date) = fit_columns(300.0, 300.0, 300.0);
        assert!(date == 0.0, "the date should go first, got {date}");
        assert!(size >= MIN_SIZE_COL, "the size should stay, got {size}");
    }

    #[test]
    fn a_very_narrow_pane_drops_the_columns_entirely() {
        let (size, date) = fit_columns(200.0, 92.0, 132.0);
        assert_eq!((size, date), (0.0, 0.0), "nothing fits, so nothing shows");
    }

    #[test]
    fn widening_the_pane_brings_a_dropped_column_back() {
        // A column that was dropped is not forgotten: it returns with the room.
        let (_, narrow_date) = fit_columns(320.0, 92.0, 132.0);
        let (_, wide_date) = fit_columns(900.0, 92.0, 132.0);
        assert!(narrow_date <= wide_date);
        assert_eq!(wide_date, 132.0, "the preference is intact");
    }

    #[test]
    fn the_drag_limit_respects_the_readable_minimum() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(WIDE, 26.0));
        let l = RowLayout::new(rect, 92.0, 132.0);
        // Widening the date as far as the limit allows still leaves the name.
        let limit = col_limit(&l, 92.0);
        let at_limit = RowLayout::new(rect, 92.0, limit);
        assert!(at_limit.name_limit() - at_limit.name.x >= MIN_NAME_COL - 0.01);
    }
}
