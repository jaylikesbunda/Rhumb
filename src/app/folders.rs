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

impl Rhumb {
    /// The folder the second pane opens at: the one above the live folder, so
    /// the two lists start side by side in the same tree. A root has no parent
    /// to step to, so the second list starts where the live one is.
    fn second_start_dir(&self) -> PathBuf {
        match self.cwd.parent() {
            Some(p) if p != self.cwd.as_path() => p.to_path_buf(),
            _ => self.cwd.clone(),
        }
    }

    /// Turns the dual-pane view on or off. Turning it on makes sure there is a
    /// second list to show.
    pub(super) fn toggle_dual(&mut self) {
        self.dual = !self.dual;
        if self.dual {
            self.ensure_second();
        }
    }

    /// Makes the second pane, at the folder above the live one, and starts its
    /// listing. A no-op once it exists, so the view can be turned off and on
    /// again without losing where the second list was.
    pub(super) fn ensure_second(&mut self) {
        if self.second.is_some() {
            return;
        }
        let start = self.second_start_dir();
        self.second = Some(SecondPane::new(start, self.view));
        self.request_listing_for_second();
    }

    /// Swaps the live list's state with the parked one's.
    ///
    /// This is what lets the one `list_ui` draw either pane, and what hands a
    /// pane the keyboard when it is clicked. It moves values only: nothing is
    /// re-read, and no entry is cloned.
    fn swap_second_state(&mut self, sec: &mut SecondPane) {
        std::mem::swap(&mut self.cwd, &mut sec.cwd);
        std::mem::swap(&mut self.entries, &mut sec.entries);
        std::mem::swap(&mut self.view, &mut sec.view);
        std::mem::swap(&mut self.visible, &mut sec.visible);
        std::mem::swap(&mut self.listing, &mut sec.listing);
        std::mem::swap(&mut self.req, &mut sec.req);
        std::mem::swap(&mut self.history, &mut sec.history);
        std::mem::swap(&mut self.cursor, &mut sec.cursor);
        std::mem::swap(&mut self.sel, &mut sec.sel);
        std::mem::swap(&mut self.anchor, &mut sec.anchor);
        std::mem::swap(&mut self.scroll_to, &mut sec.scroll_to);
        std::mem::swap(&mut self.filter, &mut sec.filter);
        std::mem::swap(&mut self.scope, &mut sec.scope);
        std::mem::swap(&mut self.groups, &mut sec.groups);
        std::mem::swap(&mut self.row_cache, &mut sec.row_cache);
        std::mem::swap(&mut self.list_scroll, &mut sec.list_scroll);
        std::mem::swap(&mut self.scroll_restore, &mut sec.scroll_restore);
        std::mem::swap(&mut self.restore_anchor, &mut sec.restore_anchor);
        std::mem::swap(&mut self.search, &mut sec.search);
        std::mem::swap(&mut self.search_shown, &mut sec.search_shown);
        std::mem::swap(&mut self.search_typed, &mut sec.search_typed);
        std::mem::swap(&mut self.typeahead, &mut sec.typeahead);
    }

    /// Re-reads the parked pane's folder, using the live path's own request so
    /// the two can never drift apart.
    pub(super) fn request_listing_for_second(&mut self) {
        let Some(mut sec) = self.second.take() else {
            return;
        };
        self.swap_second_state(&mut sec);
        self.request_listing();
        self.swap_second_state(&mut sec);
        self.second = Some(sec);
    }

    /// Makes the parked pane the live one, keeping it on the side it is drawn.
    ///
    /// Called when a press lands in the pane that is not live, so the click is
    /// handled by the list it landed in. The side flag flips with the states, so
    /// neither folder moves across the screen.
    pub(super) fn activate_second(&mut self) {
        if self.second.is_none() {
            return;
        }
        let mut sec = self.second.take().expect("checked above");
        self.swap_second_state(&mut sec);
        self.second = Some(sec);
        self.live_on_left = !self.live_on_left;
        // The tab strip names the folder on screen; the live pane just changed.
        self.sync_folder_tab();
    }

    /// Hands a finished listing to the parked pane when it is the one that
    /// asked, and says whether it was.
    ///
    /// The pane is swapped in so the same code applies the listing either way,
    /// then swapped back. A listing for neither pane is simply dropped.
    pub(super) fn deliver_second_listing(
        &mut self,
        token: u64,
        path: &Path,
        entries: Vec<Entry>,
        error: Option<String>,
    ) -> bool {
        let Some(mut sec) = self.second.take() else {
            return false;
        };
        if token != sec.req || path != sec.cwd {
            self.second = Some(sec);
            return false;
        }
        self.swap_second_state(&mut sec);
        self.apply_listing(entries, error);
        self.swap_second_state(&mut sec);
        self.second = Some(sec);
        true
    }

    /// The central area in dual-pane view: two folder lists side by side, split
    /// by a draggable divider.
    ///
    /// The live pane is whichever was pressed last; the other is swapped in to
    /// be drawn and swapped back, so both lists work with clicks, scrolling and
    /// the context menu exactly as the single list does.
    pub(super) fn dual_list_ui(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let divider_w = 7.0f32;
        let usable = (rect.width() - divider_w).max(1.0);
        let left_w = split_left(usable, self.dual_split);
        let left = Rect::from_min_size(rect.min, Vec2::new(left_w, rect.height()));
        let divider = Rect::from_min_size(
            Pos2::new(rect.min.x + left_w, rect.min.y),
            Vec2::new(divider_w, rect.height()),
        );
        let right = Rect::from_min_max(Pos2::new(divider.max.x, rect.min.y), rect.max);

        // A press in the pane that is not live makes it live, before either is
        // drawn, so the click lands in the list it was aimed at.
        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
            let pressed =
                ui.input(|i| i.pointer.primary_pressed() || i.pointer.secondary_pressed());
            let parked = if self.live_on_left { right } else { left };
            if pressed && parked.contains(p) {
                self.activate_second();
            }
        }

        let (live_rect, parked_rect) = if self.live_on_left {
            (left, right)
        } else {
            (right, left)
        };
        let mut live_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(live_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT))
                .id_salt("dual-live"),
        );
        self.list_ui(&mut live_ui, ID_LIST);

        if let Some(mut sec) = self.second.take() {
            self.swap_second_state(&mut sec);
            let mut parked_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(parked_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT))
                    .id_salt("dual-second"),
            );
            self.list_ui(&mut parked_ui, ID_LIST_SECOND);
            self.swap_second_state(&mut sec);
            self.second = Some(sec);
        }

        let resp = ui.interact(
            divider,
            Id::new("dual-split"),
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
            // Drag in pixels and convert back through the same clamp the editor
            // split uses, so a grab at the limit still moves smoothly.
            self.dual_split = split_fraction(usable, left_w + resp.drag_delta().x);
        }
    }
}
