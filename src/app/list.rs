//! The file list, its context menu, the details pane, and moving about in it.

use super::*;

impl Xplor {
    /// The file list. One code path serves all three views: only the cell
    /// geometry and the painter change.
    pub(super) fn list_ui(&mut self, ui: &mut Ui) {
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
        let rows_total = count.div_ceil(cols);
        let name_w = if grid {
            tile_w - sp::SM * 2.0
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

        let list_offset = {
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
            } else if let Some(y) = self.scroll_restore.take() {
                // A tab has come back, and the list goes to where it was.
                area = area.vertical_scroll_offset(y);
            }
            let shown = area.show(ui, |ui| {
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
                                self.thumbs.get(&entry.path, thumb_px)
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
                            // Views without columns never built a layout, so
                            // their icon and name rects were zero and every row
                            // painted at the panel's edge. Build one from the
                            // cell itself: only its x and width matter here.
                            let plain = RowLayout::new(cell, 0.0, 0.0);
                            let rl = if view.has_columns() { layout } else { plain };
                            let room = (rl.name_limit() - rl.name.x).max(40.0);
                            let galleys: &widgets::RowGalleys =
                                self.row_cache.get_or_build(ui, i, entry, room);
                            widgets::paint_row(
                                ui,
                                entry,
                                &rl,
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
                        // The menu opens on the press, with the release as a
                        // fallback. A press is visible globally, while a click
                        // needs egui to credit this exact widget — credit the
                        // row loses whenever anything overlaps it. The close
                        // behaviour below ignores the opening click, so opening
                        // early cannot dismiss the menu again.
                        let right_pressed =
                            resp.hovered() && ui.input(|i| i.pointer.secondary_pressed());
                        if right_pressed || resp.secondary_clicked() {
                            context = Some(entry.path.clone());
                            click = Some((i, ClickKind::Plain));
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
            self.drag_payload = self.sel.iter().cloned().collect();
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
            ClickKind::Open => self.open_path(&path),
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
        let dummy = ui.interact(Rect::from_min_size(anchor, Vec2::ZERO), id, Sense::hover());

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
                if item(ui, Icon::Glyph(ph::PENCIL_LINE), "Rename", "F2", false) {
                    action = Some(CtxAction::Rename);
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
        // folder glyph colliding with the path text — nothing shared an axis.
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
            0.0,
        );

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
            self.open_path(&p);
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
        // the disk to walk, or to index.
        if archive::is_virtual(&self.cwd) {
            return;
        }
        // Nothing queued is the common case, and this runs every frame. A
        // gate that only rejected *young* requests fell through to `start` here
        // and restarted the walk on every frame, clearing the results the
        // previous walk was still delivering — so the list showed "Searching"
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

    /// Rebuilds the visible index list from the filter.
    pub(super) fn recompute_visible(&mut self) {
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
        if self.sel.is_empty() {
            self.cursor_path().into_iter().collect()
        } else {
            self.sel.iter().cloned().collect()
        }
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
