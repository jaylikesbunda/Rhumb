//! The sidebar: Quick access, the drives and the folder tree.

use super::*;
use egui::{WidgetInfo, WidgetType};

impl Rhumb {
    /// Adds a folder to Quick access, telling the user why if it cannot.
    ///
    /// Returns whether the list changed, so the caller can say so.
    pub(super) fn pin(&mut self, path: &Path) -> bool {
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
    pub(super) fn unpin(&mut self, path: &Path) -> bool {
        if !self.pins.remove(path) {
            return false;
        }
        self.write_prefs();
        true
    }

    pub(super) fn is_pinned(&self, path: &Path) -> bool {
        self.pins.contains(path)
    }

    // ---- tabs -----------------------------------------------------------

    /// Explorer-style tree: quick access and drives, each expandable to reveal
    /// subfolders. Children are read lazily on a worker thread, so expanding
    /// never blocks the UI.
    pub(super) fn sidebar_ui(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let mut nav: Option<PathBuf> = None;
        let mut toggle: Option<PathBuf> = None;
        let mut expand: Option<PathBuf> = None;

        // Roots: quick access, then devices. Labels stay friendly; the tree
        // itself only cares about paths. Worked out before the scroll area, so
        // the borrow of the tree ends before the closure takes `self`.
        let roots: Vec<(String, PathBuf, bool)> = self.roots.get(ui.ctx()).to_vec();
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

        // One scroll area for the whole column: with several pinned folders or
        // an expanded tree the rows are taller than a small window, and clipping
        // them would hide folders with no way to reach them.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Quick access sits above everything else, the way Explorer puts
                // it. The section only appears once something is pinned, so an
                // unused sidebar stays quiet.
                if !self.pins.is_empty() {
                    self.quick_access_ui(ui, &mut nav);
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
                        self.open_context_menu(ui, &path);
                    }
                }
            });

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
    pub(super) fn quick_access_ui(&mut self, ui: &mut Ui, nav: &mut Option<PathBuf>) {
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
                icon.right() + sp::XS + 4.0,
                rect.center().y - g.size().y * 0.5,
            ),
            &g,
            c::TEXT_GHOST,
        );

        let mut unpin: Option<PathBuf> = None;
        for path in self.pins.clone() {
            let label = path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            if self.pin_row(ui, &label, &path, nav, &mut unpin) {
                // Same menu as everywhere else, so it offers to unpin.
                self.open_context_menu(ui, &path);
            }
        }
        if let Some(path) = unpin
            && self.unpin(&path)
        {
            self.toast(String::from("Removed from Quick access"));
        }
        ui.add_space(sp::XS);
    }

    /// One pinned row. Returns whether the row asked to be unpinned, via a
    /// right click.
    pub(super) fn pin_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        path: &Path,
        nav: &mut Option<PathBuf>,
        unpin: &mut Option<PathBuf>,
    ) -> bool {
        let active = self.cwd == path;
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(width, sp::NAV_ROW),
            Sense::click().union(Sense::drag()),
        );
        // A pinned folder is a selectable button named by its label.
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, active, label));
        // The button that takes the folder off the list, on the right of the row and
        // only while the row is under the pointer. The room for it is kept whether it
        // is showing or not, so the name does not move when it appears.
        let x_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 16.0, rect.center().y),
            Vec2::splat(20.0),
        );
        let x_resp = ui.interact(x_rect, Id::new(("unpin", path)), Sense::click());
        x_resp.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, "Remove from Quick access")
        });
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
                Pos2::new(rect.left() + 14.0, rect.center().y),
                Vec2::splat(13.0),
            );
            let icon_color = if active { c::SEL_TEXT } else { c::TEXT_FAINT };
            Icon::Pin.paint(painter, icon, icon_color);
            Icon::Folder.paint(
                painter,
                Rect::from_center_size(
                    Pos2::new(icon.right() + PIN_GAP + 6.5, rect.center().y),
                    Vec2::splat(13.0),
                ),
                icon_color,
            );
            let text_x = icon.right() + PIN_GAP + 13.0 + sp::XS + 2.0;
            let text_color = if active { c::SEL_TEXT } else { c::TEXT_DIM };
            let g = widgets::layout_elided(
                ui,
                label.to_owned(),
                theme::ui_font(tfs::BODY),
                text_color,
                (rect.right() - text_x - 32.0).max(20.0),
            );
            widgets::galley_at(
                painter,
                Pos2::new(text_x, rect.center().y - g.size().y * 0.5),
                &g,
                text_color,
            );
            if resp.hovered() || x_resp.hovered() {
                if x_resp.hovered() {
                    painter.rect_filled(x_rect, CornerRadius::same(4), c::SEL);
                }
                Icon::Close.paint(
                    painter,
                    x_rect,
                    if x_resp.hovered() {
                        c::TEXT
                    } else {
                        c::TEXT_FAINT
                    },
                );
            }
        }
        let x_resp = x_resp.on_hover_text("Remove from Quick access");
        if x_resp.clicked() {
            *unpin = Some(path.to_path_buf());
        } else if resp.clicked() {
            *nav = Some(path.to_path_buf());
        }
        resp.secondary_clicked()
    }

    /// One sidebar line: chevron, icon, label, optional free-space note.
    ///
    /// Actions are collected into the out-parameters so the caller can apply
    /// them once layout is done.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn tree_row(
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
        // "This PC" and the Recycle Bin name no folder on disk, so there is
        // nothing under them to expand: no chevron, and a click anywhere on the
        // row navigates.
        let magic = crate::recycle::is_root(path) || crate::this_pc::is_root(path);
        let width = ui.available_width();
        let indent = depth as f32 * sp::INDENT;
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(width, sp::NAV_ROW),
            Sense::click().union(Sense::drag()),
        );
        // A tree row is a selectable button named by its label; a drive or folder
        // is not distinguishable to a screen reader otherwise.
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, active, label));
        if resp.hovered() && !is_device && !magic {
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
                Icon::Refresh.paint(painter, chevron, c::TEXT_GHOST);
            } else if !is_device && !magic {
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
            let note = if is_device {
                self.free_space
                    .get(path, ui.ctx())
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
                // Readable on both grounds: a note the colour of the panel's own border
                // was there and could not be seen.
                let note_color = if active {
                    c::SEL_TEXT.gamma_multiply(0.7)
                } else {
                    c::TEXT_FAINT
                };
                let mg = widgets::layout_elided(
                    ui,
                    note,
                    theme::ui_font(tfs::SMALL),
                    note_color,
                    note_w,
                );
                widgets::text_right(
                    painter,
                    Pos2::new(rect.right() - sp::SM, rect.center().y),
                    &mg,
                    note_color,
                );
            }
        }

        // The leading chevron area expands; the rest of the row navigates. A
        // magic root has no chevron, so its whole row navigates.
        if resp.clicked() {
            let on_chevron = !magic
                && ui.input(|i| {
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
        // A right click asks for the shared context menu, as the list's rows do.
        resp.secondary_clicked()
    }

    /// Asks for a folder's subfolders, read on a worker thread.
    pub(super) fn load_tree_children(&mut self, path: &Path) {
        self.sidebar_tree.loading = Some(path.to_path_buf());
        self.sidebar_tree.tree.set_pending(path.to_path_buf());
        let tx = self.tx.clone();
        let path = path.to_path_buf();
        let _ = std::thread::Builder::new()
            .name("rhumb-tree".into())
            .spawn(move || {
                let dirs = tree::read_dirs(&path);
                let _ = tx.send(Msg::TreeLoaded { path, dirs });
            });
    }

    /// Expands or collapses a folder, loading children the first time.
    pub(super) fn expand_tree(&mut self, path: &Path) {
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
}
