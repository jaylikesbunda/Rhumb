//! Folder tabs: several places to browse at once.
//!
//! There are two kinds of thing here and they are kept apart. The *tabs along the top* are
//! folders: each is a place with its own way back and forward, filter, selection and scroll
//! position, and with its own open files. The *tabs above the editor* are the files open in
//! the folder tab that is showing. Switching folder tab swaps the whole of it: the list, and
//! the files with it.
//!
//! The folder tab on screen keeps its state in the app itself, as it always did; the others
//! are parked here, and swapped in when they are come back to.

use super::*;

/// What a folder tab that is not on screen holds.
pub(super) struct Parked {
    pub view: FolderView,
    pub docs: Tabs,
}

/// One tab along the top.
pub(super) struct FolderTab {
    pub id: u64,
    /// The folder it shows, which names it.
    pub path: PathBuf,
    /// Everything about it while it is not the one on screen. `None` for the one that is.
    pub parked: Option<Parked>,
}

impl FolderTab {
    pub fn new(path: &Path) -> FolderTab {
        FolderTab {
            id: next_tab_id(),
            path: path.to_path_buf(),
            parked: None,
        }
    }

    pub fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                // A drive: `C:` and not nothing.
                let s = self.path.to_string_lossy();
                let t = s.trim_end_matches(['\\', '/']);
                if t.is_empty() {
                    "/".to_owned()
                } else {
                    t.to_owned()
                }
            })
    }
}

impl Rhumb {
    /// Everything about the folder on screen that a tab keeps.
    pub(super) fn folder_view(&self) -> FolderView {
        FolderView {
            cwd: self.cwd.clone(),
            history: self.history.clone(),
            filter: self.filter.clone(),
            scope: Some(self.scope),
            sel: self.sel.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
            scroll: self.list_scroll,
        }
    }

    /// Starts with one tab, for the folder the window opened at.
    pub(super) fn init_folders(&mut self) {
        if self.folders.is_empty() {
            self.folders = vec![FolderTab::new(&self.cwd)];
            self.active_folder = 0;
        }
    }

    /// The folder tab on screen is named for the folder now being shown.
    pub(super) fn sync_folder_tab(&mut self) {
        let cwd = self.cwd.clone();
        if let Some(t) = self.folders.get_mut(self.active_folder) {
            t.path = cwd;
        }
    }

    /// Puts what is on screen away in its tab, so that another can take its place.
    fn park_live(&mut self) {
        let view = self.folder_view();
        let docs = std::mem::take(&mut self.tabs);
        if let Some(t) = self.folders.get_mut(self.active_folder) {
            t.path = view.cwd.clone();
            t.parked = Some(Parked { view, docs });
        }
    }

    /// Brings a folder tab to the front, with its list and its files as they were left.
    pub(super) fn switch_folder(&mut self, index: usize) {
        if index >= self.folders.len() || index == self.active_folder {
            return;
        }
        self.park_live();
        self.active_folder = index;
        let target = self.folders[index].parked.take();
        let fallback = self.folders[index].path.clone();
        match target {
            Some(p) => {
                self.tabs = p.docs;
                self.apply_folder_view(p.view);
            }
            None => {
                self.tabs = Tabs::default();
                if fallback != self.cwd {
                    self.navigate(&fallback);
                }
            }
        }
        // The file on screen is a different one, or none.
        self.doc_changed();
    }

    /// Puts the list back as a folder tab remembered it.
    fn apply_folder_view(&mut self, v: FolderView) {
        let same_place = v.cwd == self.cwd;
        let v_anchor = v.anchor;
        self.cwd = v.cwd;
        self.history = v.history;
        self.start_index_for_cwd();
        self.sidebar_tree.tree.reveal(&self.cwd.clone());
        self.typeahead.clear();
        self.clear_search();
        self.row_cache.clear();
        self.filter = v.filter;
        if let Some(s) = v.scope {
            self.scope = s;
        }
        self.sel = v.sel;
        self.cursor = v.cursor;
        self.anchor = v.anchor;
        self.restore_anchor = Some(v.anchor);
        self.scroll_restore = Some(v.scroll);
        self.listing = Listing::Loading;
        if same_place {
            // The entries on hand are this folder's: show them while it is re-read.
            self.recompute_visible();
            self.anchor = v_anchor;
        } else {
            self.entries.clear();
            self.visible.clear();
        }
        self.request_listing();
        if !self.filter.trim().is_empty() {
            self.on_filter_changed();
        }
    }

    /// Opens a new folder tab at the folder on screen, and shows it. It starts clean: no
    /// filter, nothing selected, no files, and a way back and forward of its own.
    pub(super) fn new_folder_tab(&mut self) {
        if self.folders.len() >= MAX_TABS {
            self.toast(format!("At most {MAX_TABS} tabs: close one first"));
            return;
        }
        let mut view = self.folder_view();
        view.filter.clear();
        view.sel.clear();
        view.cursor = 0;
        view.anchor = 0;
        view.scroll = 0.0;
        let mut tab = FolderTab::new(&self.cwd);
        tab.parked = Some(Parked {
            view,
            docs: Tabs::default(),
        });
        self.folders.push(tab);
        let i = self.folders.len() - 1;
        self.switch_folder(i);
    }

    /// Whether the files open in a folder tab have anything unsaved.
    pub(super) fn folder_has_unsaved(&self, index: usize) -> bool {
        if index == self.active_folder {
            return self.tabs.first_dirty().is_some();
        }
        self.folders
            .get(index)
            .and_then(|t| t.parked.as_ref())
            .is_some_and(|p| p.docs.first_dirty().is_some())
    }

    /// Closes a folder tab, and the files open in it. One with unsaved changes is not
    /// closed: it is brought forward, so they can be saved or closed first.
    pub(super) fn close_folder_tab(&mut self, index: usize) {
        if self.folders.len() <= 1 || index >= self.folders.len() {
            return;
        }
        if self.folder_has_unsaved(index) {
            self.switch_folder(index);
            self.toast("Save or close the files in this tab first".into());
            return;
        }
        if index == self.active_folder {
            // Another comes forward, and then this one is taken away from behind it.
            let next = if index + 1 < self.folders.len() {
                index + 1
            } else {
                index - 1
            };
            self.switch_folder(next);
            self.folders.remove(index);
            self.active_folder = if next > index { next - 1 } else { next };
        } else {
            self.folders.remove(index);
            if index < self.active_folder {
                self.active_folder -= 1;
            }
        }
    }

    /// Moves a folder tab to another place along the top, the one on screen staying on
    /// screen.
    pub(super) fn move_folder_tab(&mut self, from: usize, to: usize) {
        let n = self.folders.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let shown = self.folders[self.active_folder].id;
        let tab = self.folders.remove(from);
        self.folders.insert(to, tab);
        if let Some(i) = self.folders.iter().position(|t| t.id == shown) {
            self.active_folder = i;
        }
    }

    /// The next folder tab, or the one before, wrapping round.
    pub(super) fn cycle_folder(&mut self, back: bool) {
        let n = self.folders.len();
        if n < 2 {
            return;
        }
        let next = if back {
            (self.active_folder + n - 1) % n
        } else {
            (self.active_folder + 1) % n
        };
        self.switch_folder(next);
    }

    /// The row of folder tabs along the top of the window.
    pub(super) fn folder_strip_ui(&mut self, ui: &mut Ui, rect: Rect) {
        let items: Vec<strip::Item> = self
            .folders
            .iter()
            .enumerate()
            .map(|(i, t)| strip::Item {
                id: t.id,
                label: t.label(),
                icon: Icon::Folder,
                dirty: self.folder_has_unsaved(i),
            })
            .collect();
        let action = strip::show(
            ui,
            rect,
            &items,
            self.active_folder,
            &mut self.folder_strip,
            &strip::Style {
                salt: "folder-tabs",
                new_tip: Some("New tab (Ctrl+T)"),
                min_w: 104.0,
                max_w: 200.0,
            },
        );
        if let Some((from, to)) = action.reorder {
            self.move_folder_tab(from, to);
        }
        if action.add {
            self.new_folder_tab();
        }
        if let Some(i) = action.focus.or(action.pick) {
            self.switch_folder(i);
        }
        if let Some(i) = action.close {
            self.close_folder_tab(i);
        }
    }

    /// How many folder tabs there are.
    #[cfg(test)]
    pub(super) fn folder_tab_count(&self) -> usize {
        self.folders.len()
    }
}
