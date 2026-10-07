//! The file list, its context menu, the details pane, and moving about in it.

use super::*;
use egui::{WidgetInfo, WidgetType};
use std::time::SystemTime;

/// The room the details pane's close button takes from the title row, in points.
const CLOSE_W: f32 = 28.0;

impl Rhumb {
    /// The file list. One code path serves all three views: only the cell
    /// geometry and the painter change.
    ///
    /// `pane` is a widget-id salt, so the live list and the parked second list
    /// can be drawn in the same frame without their rows, headers and scroll
    /// areas sharing ids.
    pub(super) fn list_ui(&mut self, ui: &mut Ui, pane: &'static str) {
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
                let resp = ui.interact(r, Id::new((pane, "sort", key.label())), Sense::click());
                resp.widget_info(|| {
                    WidgetInfo::labeled(
                        WidgetType::Button,
                        ui.is_enabled(),
                        format!("Sort by {}", key.label()),
                    )
                });
                if resp.clicked() {
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
                let resp = ui.interact(grab, Id::new((pane, id)), Sense::drag());
                resp.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::ResizeHandle, true, "Resize column")
                });
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
            } else if crate::recycle::is_root(&self.cwd) {
                // Not a folder that happens to be empty: the bin is a place of
                // its own, and saying so reads better than "Empty folder".
                ("Recycle Bin is empty", "Deleted items show up here")
            } else if crate::this_pc::is_root(&self.cwd) {
                (
                    "Nothing to show",
                    "This PC lists the drives and your folders",
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
        let tile_scale = 0.8 + 1.6 * self.zoom;
        let tile_w = (sp::TILE_W * tile_scale).round();
        let cell_h = if grid {
            (sp::TILE * tile_scale).round()
        } else {
            (sp::ROW * (0.85 + 0.6 * self.zoom)).round()
        };
        let thumb_px = thumbs::bucket(tile_w - sp::SM * 2.0, ui.ctx().pixels_per_point());
        let cols = if grid {
            ((width - sp::SM) / tile_w).floor().max(1.0) as usize
        } else {
            1
        };
        let name_w = if grid {
            tile_w - sp::SM * 2.0
        } else if view.has_columns() {
            layout.name_limit() - layout.name.x
        } else {
            (width - 74.0).max(40.0)
        };
        // Grouping is a property of the folder listing, not of a recursive
        // search: results are shown flat whatever the grouping is.
        let grouped = self.group_by != GroupBy::None && !searching;
        let header_h = sp::SECTION;
        // The scroll area's content is as tall as its rows. A group header
        // takes a fixed strip above the rows of its group.
        let content_h = if grouped {
            self.groups
                .iter()
                .map(|g| header_h + group_rows(g.len, grid, cols) as f32 * cell_h)
                .sum()
        } else {
            count.div_ceil(cols) as f32 * cell_h
        };

        let mut click: Option<(usize, ClickKind)> = None;
        // Collected inside the loop and applied after it: the closure borrows
        // list fields, so it cannot reach into the app to open a menu itself.
        let mut context: Option<PathBuf> = None;
        let scroll_to = self.scroll_to.take();
        // The cursor is an item index. In the grouped layout a header may sit
        // above it, so the scroll target is that item's actual row, not its
        // index.
        let scroll_offset = scroll_to.map(|(item, _align)| {
            if grouped {
                grouped_item_offset(&self.groups, item, grid, cols, header_h, cell_h)
            } else {
                item as f32 * cell_h
            }
        });

        let list_offset = {
            // Immutable list data for the paint loop; the caches are separate
            // fields, so the borrow checker lets us touch them here.
            let entries = &self.entries;
            let visible = &self.visible;
            let results = &self.search.results;
            let groups = &self.groups;
            let sel = &self.sel;

            let mut area = egui::ScrollArea::vertical()
                .id_salt((pane, ID_LIST))
                .auto_shrink([false, false]);
            if let Some(y) = scroll_offset {
                area = area.vertical_scroll_offset(y);
            } else if let Some(y) = self.scroll_restore.take() {
                // A tab has come back, and the list goes to where it was.
                area = area.vertical_scroll_offset(y);
            }
            let shown = area.show(ui, |ui| {
                let (content, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width().max(1.0), content_h.max(1.0)),
                    Sense::hover(),
                );
                let clip = ui.clip_rect();
                if grouped {
                    // Walk the groups rather than the rows: there are only a
                    // handful of groups, so finding the first visible row costs
                    // nothing however many entries a group holds.
                    let top = (clip.min.y - content.min.y).max(0.0);
                    let bottom = top + clip.height();
                    let mut y = 0.0f32;
                    for g in groups {
                        let header_y = y;
                        y += header_h;
                        let rows = group_rows(g.len, grid, cols);
                        let items_bottom = y + rows as f32 * cell_h;
                        // Entirely above the viewport: skip it whole.
                        if items_bottom < top {
                            y = items_bottom;
                            continue;
                        }
                        // Entirely below: so is everything after it.
                        if y > bottom {
                            break;
                        }
                        if header_y + header_h > top && header_y < bottom {
                            let rect = Rect::from_min_size(
                                Pos2::new(content.min.x, content.min.y + header_y),
                                Vec2::new(content.width(), header_h),
                            );
                            widgets::group_header(ui, rect, &g.label);
                        }
                        let first = (((top - y).max(0.0)) / cell_h).floor() as usize;
                        let last =
                            ((((bottom - y).max(0.0)) / cell_h).ceil() as usize + 1).min(rows);
                        for line in first..last {
                            for c in 0..cols {
                                let k = line * cols + c;
                                if k >= g.len {
                                    break;
                                }
                                let item = g.start + k;
                                let Some(entry) = visible.get(item).and_then(|e| entries.get(*e))
                                else {
                                    continue;
                                };
                                let cell = if grid {
                                    Rect::from_min_size(
                                        Pos2::new(
                                            content.min.x + c as f32 * tile_w,
                                            content.min.y + y + line as f32 * cell_h,
                                        ),
                                        Vec2::new(tile_w, cell_h),
                                    )
                                } else {
                                    Rect::from_min_size(
                                        Pos2::new(
                                            content.min.x,
                                            content.min.y + y + line as f32 * cell_h,
                                        ),
                                        Vec2::new(content.width(), cell_h),
                                    )
                                };
                                let selected = sel.contains(&entry.path);
                                let resp = paint_item(
                                    ui,
                                    entry,
                                    item,
                                    cell,
                                    grid,
                                    view,
                                    &layout,
                                    selected,
                                    name_w,
                                    thumb_px,
                                    pane,
                                    &mut self.row_cache,
                                    &mut self.thumbs,
                                    &mut self.drop_target,
                                );
                                if resp.clicked() {
                                    click = Some((item, click_kind(ui)));
                                }
                                if resp.double_clicked() {
                                    click = Some((item, ClickKind::Open));
                                }
                                let right_pressed =
                                    resp.hovered() && ui.input(|i| i.pointer.secondary_pressed());
                                if right_pressed || resp.secondary_clicked() {
                                    context = Some(entry.path.clone());
                                    // A right click inside the selection keeps it,
                                    // so the menu can act on the whole of it.
                                    click = Some((
                                        item,
                                        if sel.contains(&entry.path) {
                                            ClickKind::Context
                                        } else {
                                            ClickKind::Plain
                                        },
                                    ));
                                }
                            }
                        }
                        y = items_bottom;
                    }
                } else {
                    let scrolled = clip.min.y - content.min.y;
                    let rows_total = count.div_ceil(cols);
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
                                        content.min.x + c as f32 * tile_w,
                                        content.min.y + r as f32 * cell_h,
                                    ),
                                    Vec2::new(tile_w, cell_h),
                                )
                            } else {
                                Rect::from_min_size(
                                    Pos2::new(content.min.x, content.min.y + i as f32 * cell_h),
                                    Vec2::new(content.width(), cell_h),
                                )
                            };
                            let selected = sel.contains(&entry.path);
                            let resp = paint_item(
                                ui,
                                entry,
                                i,
                                cell,
                                grid,
                                view,
                                &layout,
                                selected,
                                name_w,
                                thumb_px,
                                pane,
                                &mut self.row_cache,
                                &mut self.thumbs,
                                &mut self.drop_target,
                            );
                            if resp.clicked() {
                                click = Some((i, click_kind(ui)));
                            }
                            if resp.double_clicked() {
                                click = Some((i, ClickKind::Open));
                            }
                            // The menu opens on the press, with the release as a
                            // fallback. A press is visible globally, while a click
                            // needs egui to credit this exact widget - credit the
                            // row loses whenever anything overlaps it. The close
                            // behaviour below ignores the opening click, so opening
                            // early cannot dismiss the menu again.
                            let right_pressed =
                                resp.hovered() && ui.input(|i| i.pointer.secondary_pressed());
                            if right_pressed || resp.secondary_clicked() {
                                context = Some(entry.path.clone());
                                // A right click inside the selection keeps it,
                                // so the menu can act on the whole of it.
                                click = Some((
                                    i,
                                    if sel.contains(&entry.path) {
                                        ClickKind::Context
                                    } else {
                                        ClickKind::Plain
                                    },
                                ));
                            }
                        }
                    }
                }
            });
            shown.state.offset.y
        };
        self.list_scroll = list_offset;

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
            // A deleted item cannot be dragged anywhere: the only thing to do
            // with one is restore it, and a drop would move the raw `$R` file.
            if !self.sel.iter().any(|p| crate::recycle::is_item(p)) {
                self.drag_payload = self.sel.iter().cloned().collect();
            }
        } else if !self.drag_payload.is_empty() && !ui.input(|i| i.pointer.any_down()) {
            self.drag_payload.clear();
        }
        self.perf.list_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    pub(super) fn handle_click(&mut self, i: usize, kind: ClickKind) {
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
            ClickKind::Open => {
                // A deleted item is not opened where it lies; the useful thing
                // a double-click can do is put it back.
                if crate::recycle::is_item(&path) {
                    self.restore_recycle(&path);
                } else {
                    self.open_path(&path);
                }
            }
            ClickKind::Context => {
                // Right-clicking inside an existing selection keeps it: the
                // menu is about to offer an action for all of it.
                self.cursor = i;
                self.anchor = i;
            }
        }
    }

    /// Records a right click so the menu opens on this frame.
    ///
    /// Both the click position and the path are captured here. The position has
    /// to be remembered rather than read per frame, or the menu would slide
    /// along behind the pointer as it moves.
    pub(super) fn open_context_menu(&mut self, ui: &Ui, path: &Path) {
        let anchor = ui.input(|i| i.pointer.interact_pos());
        self.menu.open(anchor, path.to_path_buf());
    }

    /// Right-click menu.
    ///
    /// The popup is anchored to the pointer, not to the `Ui` cursor. The list is
    /// virtualized, so by the time we get here the cursor can be millions of
    /// pixels below the viewport, and a menu anchored there opens off-screen.
    pub(super) fn context_menu(&mut self, ui: &mut Ui, path: &Path) {
        let path = path.to_path_buf();
        let is_dir = path.is_dir();
        // Every file can be opened in the editor now, so this is only used to
        // decide whether the item reads as text.
        let editable = !is_dir;
        let pinned = self.is_pinned(&path);
        // An archive, or something inside one, can be extracted.
        let is_archive = archive::is_archive_file(&path) || archive::is_virtual(&path);
        // A deleted item has only the two things that can be done with it.
        let is_recycle = crate::recycle::is_item(&path);
        // Rename acts on the whole selection when the menu was opened on one of
        // several selected rows; the right-click kept that selection for us.
        let batch = self.sel.len() >= 2 && self.sel.contains(&path);
        let mut action: Option<CtxAction> = None;

        // The anchor is the spot that was clicked, recorded when the right
        // click landed. Reading the pointer every frame instead would drag the
        // menu along behind the mouse.
        let anchor = self.menu.anchor().unwrap_or_else(|| {
            ui.input(|i| i.pointer.interact_pos())
                .unwrap_or_else(|| ui.max_rect().center())
        });
        // Keyed by the opening, not the path, so each menu gets an id egui has
        // never drawn before. See `open_context_menu`.
        let id = self.menu.id();
        // Hover-only, so this placeholder can never steal the click credit
        // from the row beneath it. It exists only to give the popup an id, a
        // layer, and a rect to sit beside.
        let dummy = ui.interact(
            Rect::from_min_size(anchor, Vec2::ZERO),
            id.with("anchor"),
            Sense::hover(),
        );

        let popup = egui::Popup::menu(&dummy).id(id);
        // Open exactly once, on the frame the right click landed. After that
        // egui owns the state: it closes on a click outside or on Escape, and
        // we must not re-open behind its back or the menu could never be
        // dismissed. Clicks *inside* the menu do not close it, so picking an
        // item closes it by hand below.
        let open = self
            .menu
            .take_open()
            .then_some(egui::containers::SetOpenCommand::Bool(true));

        let shown = popup
            .open_memory(open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(240.0)
            .show(|ui| {
                use egui_phosphor::regular as ph;
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.set_min_width(240.0);
                ui.add_space(4.0);
                let item = |ui: &mut Ui, icon: Icon, label: &str, hint: &str, danger: bool| {
                    widgets::menu_item(ui, icon, label, hint, danger, true).clicked()
                };
                if is_recycle {
                    // A deleted item cannot be copied, renamed or trashed: it is
                    // already in the bin. The only choices are to put it back or
                    // to remove it for good.
                    if item(
                        ui,
                        Icon::Glyph(ph::ARROW_COUNTER_CLOCKWISE),
                        "Restore",
                        "",
                        false,
                    ) {
                        action = Some(CtxAction::Restore);
                    }
                    widgets::menu_separator(ui);
                    if item(
                        ui,
                        Icon::Glyph(ph::TRASH_SIMPLE),
                        "Delete permanently\u{2026}",
                        "",
                        true,
                    ) {
                        action = Some(CtxAction::DeleteRecycle);
                    }
                    ui.add_space(4.0);
                    return;
                }
                if editable
                    && item(
                        ui,
                        Icon::Glyph(ph::PENCIL_SIMPLE),
                        "Open in editor",
                        "",
                        false,
                    )
                {
                    action = Some(CtxAction::OpenEditor);
                }
                if item(
                    ui,
                    Icon::Glyph(ph::ARROW_SQUARE_OUT),
                    "Open with system app",
                    "",
                    false,
                ) {
                    action = Some(CtxAction::OpenExternal);
                }
                if is_dir
                    && item(
                        ui,
                        Icon::Glyph(ph::APP_WINDOW),
                        "Open in new window",
                        "",
                        false,
                    )
                {
                    action = Some(CtxAction::OpenNewWindow);
                }
                if item(
                    ui,
                    Icon::Glyph(ph::FOLDER_OPEN),
                    "Show in file manager",
                    "",
                    false,
                ) {
                    action = Some(CtxAction::Reveal);
                }
                if item(
                    ui,
                    Icon::Glyph(ph::TERMINAL_WINDOW),
                    "Open in Terminal",
                    "",
                    false,
                ) {
                    action = Some(CtxAction::OpenTerminal);
                }
                // Links are made beside the item. Nothing inside an archive has a
                // disk of its own to hold one, and a recycle item never gets here.
                if !archive::is_virtual(&path) {
                    if item(
                        ui,
                        Icon::Glyph(ph::LINK_SIMPLE),
                        "Create symbolic link",
                        "",
                        false,
                    ) {
                        action = Some(CtxAction::CreateSymlink);
                    }
                    #[cfg(windows)]
                    if is_dir
                        && item(
                            ui,
                            Icon::Glyph(ph::FOLDER_DOTTED),
                            "Create junction",
                            "",
                            false,
                        )
                    {
                        action = Some(CtxAction::CreateJunction);
                    }
                }
                if is_archive && item(ui, Icon::Glyph(ph::FILE_ZIP), "Extract here", "", false) {
                    action = Some(CtxAction::Extract);
                }
                widgets::menu_separator(ui);
                if item(ui, Icon::Glyph(ph::COPY), "Copy", "Ctrl+C", false) {
                    action = Some(CtxAction::Copy);
                }
                if item(ui, Icon::Glyph(ph::SCISSORS), "Cut", "Ctrl+X", false) {
                    action = Some(CtxAction::Cut);
                }
                if item(ui, Icon::Glyph(ph::LINK_SIMPLE), "Copy as path", "", false) {
                    action = Some(CtxAction::CopyPath);
                }
                if is_dir {
                    if pinned {
                        if item(
                            ui,
                            Icon::Glyph(ph::PUSH_PIN_SLASH),
                            "Unpin from Quick access",
                            "",
                            false,
                        ) {
                            action = Some(CtxAction::Unpin);
                        }
                    } else if item(
                        ui,
                        Icon::Glyph(ph::PUSH_PIN),
                        "Pin to Quick access",
                        "",
                        false,
                    ) {
                        action = Some(CtxAction::Pin);
                    }
                }
                if item(
                    ui,
                    Icon::Glyph(ph::PENCIL_LINE),
                    if batch { "Rename\u{2026}" } else { "Rename" },
                    "F2",
                    false,
                ) {
                    action = Some(if batch {
                        CtxAction::RenameMany
                    } else {
                        CtxAction::Rename
                    });
                }
                widgets::menu_separator(ui);
                if item(ui, Icon::Glyph(ph::TRASH), "Move to trash", "Del", true) {
                    action = Some(CtxAction::Delete);
                }
                if item(
                    ui,
                    Icon::Glyph(ph::X_CIRCLE),
                    "Delete permanently\u{2026}",
                    "Shift+Del",
                    true,
                ) {
                    action = Some(CtxAction::DeleteForever);
                }
                ui.add_space(4.0);
            });

        // `show` returns `None` once the popup is closed, which is the only
        // reliable signal: egui closes the popup *after* drawing it, so on the
        // closing frame it still answers `Some`. Dropping the path here stops
        // us asking for a menu that is no longer up.
        if shown.is_none() {
            self.menu.close();
        }

        let Some(action) = action else { return };
        // A click inside the menu does not close it on its own, so an item
        // that ran must put it away itself.
        self.menu.close();
        match action {
            CtxAction::Extract => self.start_extract(&path),
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
            CtxAction::RenameMany => {
                let paths: Vec<PathBuf> = self.sel.iter().cloned().collect();
                self.start_batch_rename(paths);
            }
            CtxAction::CreateSymlink => self.create_symlink(&path),
            #[cfg(windows)]
            CtxAction::CreateJunction => self.create_junction(&path),
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
            CtxAction::Restore => self.restore_recycle(&path),
            CtxAction::DeleteRecycle => self.delete_recycle(&path),
        }
    }

    /// Puts a Recycle Bin item back where it came from, then re-reads the bin.
    pub(super) fn restore_recycle(&mut self, path: &Path) {
        match crate::recycle::restore(path) {
            Ok(()) => {
                self.toast(String::from("Restored"));
                self.sel.clear();
                self.request_listing();
            }
            Err(e) => self.toast_err(format!("Could not restore: {e}")),
        }
    }

    /// Removes a Recycle Bin item for good, then re-reads the bin.
    pub(super) fn delete_recycle(&mut self, path: &Path) {
        match crate::recycle::delete_permanently(path) {
            Ok(()) => {
                self.toast(String::from("Deleted permanently"));
                self.sel.clear();
                self.request_listing();
            }
            Err(e) => self.toast_err(format!("Could not delete: {e}")),
        }
    }

    // ---- details pane -----------------------------------------------------

    /// The Explorer details pane: what the selection is, plus a preview of the
    /// one item under the cursor. Text shows its first lines, Markdown renders,
    /// images show the thumbnail.
    pub(super) fn details_ui(&mut self, ui: &mut Ui) {
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

        // The same centred header the editor uses: glyph and name as one
        // block, the location beneath. The old layout drew the name in one
        // strip and the path a row lower, offset by the name's width, with the
        // folder glyph colliding with the path text - nothing shared an axis.
        let width = ui.available_width();
        let header_h = HEADER_TITLE_H + HEADER_PATH_H;
        let (header, _) = ui.allocate_exact_size(Vec2::new(width, header_h), Sense::hover());
        let title_row = Rect::from_min_max(
            header.min,
            Pos2::new(header.right(), header.top() + HEADER_TITLE_H),
        );
        panel_header(
            ui,
            header,
            title_row,
            if is_dir { Icon::Folder } else { Icon::File },
            name,
            path.parent().unwrap_or(&path).to_string_lossy().to_string(),
            false,
            false,
            CLOSE_W,
        );

        // The way out: the same switch as Settings and Alt+P turn, so the pane
        // is never something that can only be got rid of by deselecting.
        let close = Rect::from_center_size(
            Pos2::new(
                title_row.right() - CLOSE_W * 0.5 - 2.0,
                title_row.center().y,
            ),
            Vec2::splat(24.0),
        );
        let resp = ui.interact(close, Id::new("details-close"), Sense::click());
        resp.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, true, "Close the details pane")
        });
        if resp.hovered() {
            ui.painter()
                .rect_filled(close, CornerRadius::same(6), c::HOVER);
        }
        Icon::Close.paint(
            ui.painter(),
            close,
            if resp.hovered() { c::TEXT } else { c::TEXT_DIM },
        );
        if resp.on_hover_text("Close the details pane").clicked() {
            self.details = false;
        }

        // Thumbnail or a centred glyph, then the facts.
        // A picture gets the width of the pane, and as many pixels as that width is on
        // this screen: a fixed 168 points from a 128 pixel decode was a smudge in a
        // corner.
        let picture = !is_dir && thumbs::is_image(&path);
        let preview_h = if picture {
            (width * 0.8).clamp(168.0, 420.0)
        } else {
            168.0f32
        };
        let (art, _) = ui.allocate_exact_size(Vec2::new(width, preview_h), Sense::hover());
        let ppp = ui.ctx().pixels_per_point();
        let thumb = if is_dir {
            None
        } else {
            self.thumbs.get(&path, thumbs::bucket(width, ppp).max(256))
        };

        let painter = ui.painter();
        match thumb {
            Some(tex) => {
                let src = tex.size_vec2();
                let room = Vec2::new(width - sp::MD, preview_h - sp::SM);
                // As big as the room allows, but never past twice its own pixels.
                let scale = (room.x / src.x).min(room.y / src.y).min(2.0 / ppp);
                let size = src * scale;
                egui::Image::new(&tex)
                    .fit_to_exact_size(size)
                    .paint_at(ui, Rect::from_center_size(art.center(), size));
            }
            None => {
                let color = if is_dir { c::TEXT } else { c::TEXT_FAINT };
                let rect = Rect::from_center_size(art.center(), Vec2::splat(64.0));
                if is_dir {
                    Icon::Folder.paint_large(painter, rect, color);
                } else {
                    Icon::File.paint_large(painter, rect, color);
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
    pub(super) fn text_preview(&mut self, ui: &mut Ui, path: &Path) {
        section_header(ui, "PREVIEW");
        let ready = self.peek.as_ref().is_some_and(|(p, _)| p == path);
        if !ready {
            if self.peek_pending.as_deref() != Some(path) {
                self.peek_pending = Some(path.to_path_buf());
                let p = path.to_path_buf();
                let tx = self.tx.clone();
                let _ = std::thread::Builder::new()
                    .name("rhumb-peek".into())
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

    /// Switches the list layout and forgets anything sized to the old one.
    pub(super) fn set_view(&mut self, mode: ViewMode) {
        if self.view == mode {
            return;
        }
        self.view = mode;
        self.row_cache.clear();
    }

    // ---- settings ----------------------------------------------------------

    pub(super) fn list_keys(&mut self, ctx: &Context) {
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
        if nav.6
            && let Some(p) = self.cursor_path()
        {
            if !self.sel.remove(&p) {
                self.sel.insert(p);
            }
            self.set_cursor(step(self.cursor, 1));
        }
        if nav.7
            && let Some(p) = self.cursor_path()
        {
            // Enter does the same as a double-click: restore, not open.
            if crate::recycle::is_item(&p) {
                self.restore_recycle(&p);
            } else {
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
    pub(super) fn typeahead_keys(&mut self, ctx: &Context, count: usize) {
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
    pub(super) fn move_cursor(&mut self, next: usize, extend: bool) {
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

    pub(super) fn set_cursor(&mut self, next: usize) {
        self.cursor = next;
        self.anchor = next;
        let align = if next == 0 {
            Align2::LEFT_TOP
        } else {
            Align2::CENTER_CENTER
        };
        self.scroll_to = Some((next, align));
    }

    pub(super) fn filter_focused(&self, ctx: &Context) -> bool {
        ctx.memory(|m| m.has_focus(Id::new(ID_SEARCH)))
    }

    // ---- editor text transforms --------------------------------------------

    // These used to live here, reaching into the text widget's cursor state to
    // run auto-indent, auto-close, indent and comment toggles after the fact.
    // The editor now owns its own caret and selection, so it applies those
    // transforms itself as the keystroke happens, while the position it needs
    // is still the position the keystroke had. Nothing here is left to do.

    // ---- messages -----------------------------------------------------------

    /// Reacts to a change in the search box: filters the current folder at once,
    /// then starts a recursive search after a short pause.
    pub(super) fn on_filter_changed(&mut self) {
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
    pub(super) fn pump_search(&mut self) {
        if self.scope != SearchScope::Below || self.filter.trim().is_empty() {
            return;
        }
        // Inside an archive the list is filtered in place: there is no folder tree on
        // the disk to walk, or to index. The Recycle Bin and "This PC" are the same
        // kind of place - magic paths with no folder behind them - so a deep search
        // there would only ask the disk about a path that names none. Their listed
        // entries are narrowed in place instead, which is what `watch` assumes too.
        if archive::is_virtual(&self.cwd)
            || crate::recycle::is_root(&self.cwd)
            || crate::this_pc::is_root(&self.cwd)
        {
            return;
        }
        // Nothing queued is the common case, and this runs every frame. A
        // gate that only rejected *young* requests fell through to `start` here
        // and restarted the walk on every frame, clearing the results the
        // previous walk was still delivering - so the list showed "Searching"
        // and never showed a single hit.
        if !search_due(self.search_typed, SEARCH_DEBOUNCE) {
            return;
        }
        self.search_typed = None;
        // Out of the index when there is one for this folder and it is finished, which
        // is an answer at once with the best match first. Otherwise the disk is walked,
        // and an index is started for next time.
        match self.indexes.ready_for(&self.cwd).cloned() {
            Some(index) => {
                self.search
                    .start_indexed(index, &self.cwd, &self.filter, self.tx.clone())
            }
            None => {
                self.indexes.ensure(&self.cwd);
                self.search.start(&self.cwd, &self.filter, self.tx.clone());
            }
        }
        self.search_shown = true;
        self.row_cache.clear();
    }

    // ---- navigation ---------------------------------------------------------

    /// Rebuilds the visible index list from the name filter and the three
    /// filter menus, then orders it into groups when a grouping is chosen.
    ///
    /// This runs on every recompute, never per frame, so a folder of a hundred
    /// thousand entries is walked only when something about the list changes.
    pub(super) fn recompute_visible(&mut self) {
        let filter = self.filter.trim();
        let now = SystemTime::now();
        let mut items: Vec<usize> = Vec::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            let named = filter.is_empty() || search::matches(&e.name, filter);
            if named
                && self.kind_filter.accepts(e)
                && self.date_filter.accepts(e, now)
                && self.size_filter.accepts(e)
            {
                items.push(i);
            }
        }
        self.visible = items;
        self.build_groups();
        let count = self.visible.len();
        if self.cursor >= count {
            self.cursor = count.saturating_sub(1);
        }
        self.anchor = self.cursor;
        self.row_cache.clear();
    }

    /// Orders the filtered items into their groups and records where each one
    /// starts. A no-op when grouping is off.
    ///
    /// The sort is stable, so within a group the list keeps the sort order the
    /// header chose; only the groups themselves are rearranged, and only when
    /// the entries happened to interleave.
    fn build_groups(&mut self) {
        self.groups.clear();
        if self.group_by == GroupBy::None {
            return;
        }
        let mut labeled: Vec<(usize, String)> = self
            .visible
            .iter()
            .map(|i| (*i, fs_model::group_label(&self.entries[*i], self.group_by)))
            .collect();
        labeled.sort_by_key(|(_, label)| fs_model::group_rank(self.group_by, label));
        self.visible.clear();
        let mut last: Option<String> = None;
        for (i, label) in labeled {
            if last.as_deref() != Some(label.as_str()) {
                self.groups.push(Group {
                    label: label.clone(),
                    start: self.visible.len(),
                    len: 0,
                });
                last = Some(label);
            }
            if let Some(g) = self.groups.last_mut() {
                g.len += 1;
            }
            self.visible.push(i);
        }
    }

    /// Whether any filter beyond the name box is narrowing the list.
    pub(super) fn filters_active(&self) -> bool {
        self.kind_filter != KindFilter::All
            || self.date_filter != DateFilter::Any
            || self.size_filter != SizeFilter::Any
    }

    /// Puts every filter back to showing everything.
    pub(super) fn clear_filters(&mut self) {
        self.kind_filter = KindFilter::All;
        self.date_filter = DateFilter::Any;
        self.size_filter = SizeFilter::Any;
        self.recompute_visible();
    }

    pub(super) fn apply_sort(&mut self) {
        fs_model::sort(&mut self.entries, self.sort, self.ascending);
        self.recompute_visible();
    }

    pub(super) fn row_count(&self) -> usize {
        if self.searching() {
            self.search.results.len()
        } else {
            self.visible.len()
        }
    }

    pub(super) fn searching(&self) -> bool {
        self.search.running || self.search_shown
    }

    pub(super) fn clear_search(&mut self) {
        self.search.cancel();
        self.search.results.clear();
        self.search.scanned = 0;
        self.search_shown = false;
        self.search_typed = None;
        self.row_cache.clear();
    }

    pub(super) fn cursor_path(&self) -> Option<PathBuf> {
        self.path_at(self.cursor)
    }

    pub(super) fn path_at(&self, i: usize) -> Option<PathBuf> {
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
    pub(super) fn target_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = if self.sel.is_empty() {
            self.cursor_path().into_iter().collect()
        } else {
            self.sel.iter().cloned().collect()
        };
        // A Recycle Bin item is not a file to copy, rename or trash: the only
        // operations that apply to one are Restore and Delete permanently,
        // which the menu offers instead. Dropping them here closes the keyboard
        // routes (Del, Shift+Del, Ctrl+C, Ctrl+X) that never reach the menu.
        paths.retain(|p| !crate::recycle::is_item(p));
        paths
    }

    pub(super) fn select_all(&mut self) {
        self.sel.clear();
        for i in 0..self.row_count() {
            if let Some(p) = self.path_at(i) {
                self.sel.insert(p);
            }
        }
    }

    // ---- opening files ---------------------------------------------------------
}

/// How many grid lines a group's items take: one row each in the list views,
/// and as many rows of `cols` as the tiles need in the grid.
fn group_rows(len: usize, grid: bool, cols: usize) -> usize {
    if grid { len.div_ceil(cols.max(1)) } else { len }
}

/// The y offset of an item in the grouped layout, so scrolling to the cursor
/// lands on it however many headers sit above it.
fn grouped_item_offset(
    groups: &[Group],
    item: usize,
    grid: bool,
    cols: usize,
    header_h: f32,
    cell_h: f32,
) -> f32 {
    let mut y = 0.0f32;
    for g in groups {
        if item < g.start + g.len {
            let k = item.saturating_sub(g.start);
            let line = if grid { k / cols.max(1) } else { k };
            return y + header_h + line as f32 * cell_h;
        }
        y += header_h + group_rows(g.len, grid, cols) as f32 * cell_h;
    }
    y
}

/// Paints one entry as a row or a tile and returns its response, so the caller
/// can read clicks and the context menu. Shared by the flat and grouped
/// layouts, and by every view.
///
/// `item` is the entry's index in the flat item list; it names the widget, so
/// an id does not move when a header is inserted above it.
#[allow(clippy::too_many_arguments)]
fn paint_item(
    ui: &mut Ui,
    entry: &Entry,
    item: usize,
    cell: Rect,
    grid: bool,
    view: ViewMode,
    layout: &RowLayout,
    selected: bool,
    name_w: f32,
    thumb_px: u32,
    pane: &'static str,
    row_cache: &mut RowCache,
    thumbs: &mut Thumbs,
    drop_target: &mut Option<PathBuf>,
) -> egui::Response {
    let resp = ui.interact(
        cell,
        Id::new((pane, "row", item)),
        Sense::click().union(Sense::drag()),
    );
    // A folder under the pointer is a drop target.
    if entry.is_dir && resp.hovered() {
        *drop_target = Some(entry.path.clone());
    }
    // The row is a selectable button whose name is the file's. The raw name is
    // borrowed, not cloned, so naming every visible row costs nothing extra.
    resp.widget_info(|| {
        WidgetInfo::selected(WidgetType::Button, true, selected, entry.name.as_str())
    });
    if grid {
        let name = row_cache.tile_name(ui, item, entry, name_w);
        let thumb = if entry.is_dir {
            None
        } else {
            thumbs.get(&entry.path, thumb_px)
        };
        // A decoded or shell-requested thumbnail is its own best icon; until
        // one arrives, or when there is none, the icon the shell shows for the
        // type stands in, and `paint_tile` draws a glyph if even that is
        // missing.
        let shell = if thumb.is_none() {
            crate::shell_icons::entry_icon(entry, ui.ctx())
        } else {
            None
        };
        widgets::paint_tile(
            ui,
            entry,
            cell,
            selected,
            resp.hovered(),
            &name,
            thumb.as_ref(),
            shell.as_ref(),
        );
    } else {
        // Views without columns never built a layout, so their icon and name
        // rects were zero and every row painted at the panel's edge. Build one
        // from the cell itself: only its x and width matter here.
        let plain = RowLayout::new(cell, 0.0, 0.0);
        let rl = if view.has_columns() { *layout } else { plain };
        let room = (rl.name_limit() - rl.name.x).max(40.0);
        let galleys: &widgets::RowGalleys = row_cache.get_or_build(ui, item, entry, room);
        let shell = crate::shell_icons::entry_icon(entry, ui.ctx());
        widgets::paint_row(
            ui,
            entry,
            &rl,
            cell,
            selected,
            resp.hovered(),
            galleys,
            view.has_columns(),
            shell.as_ref(),
        );
    }
    resp
}
