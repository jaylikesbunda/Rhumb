//! The settings window and the preferences file.

use super::*;

impl Xplor {
    /// Reads `prefs.txt` from the user config directory. Missing or malformed
    /// entries simply fall back to the defaults, so a bad file can never stop
    /// the app from starting.
    ///
    /// `start_dir` is the folder named on the command line, if any. It wins
    /// over the remembered folder, so opening a path from a shell or a file
    /// manager lands where the user asked rather than where they were last.
    pub(super) fn apply_prefs(&mut self, start_dir: Option<&Path>) {
        let Ok(text) = std::fs::read_to_string(prefs_path()) else {
            return;
        };
        self.apply_prefs_text(&text, start_dir);
    }

    /// The same, from the text of the file.
    pub(super) fn apply_prefs_text(&mut self, text: &str, start_dir: Option<&Path>) {
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
            self.sidebar_w = if (v - SIDEBAR_OLD_DEFAULT).abs() < 0.5 {
                SIDEBAR_DEFAULT
            } else {
                v.clamp(SIDEBAR_MIN, SIDEBAR_MAX)
            };
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
        if let Some(v) = b("sync_scroll") {
            self.sync_scroll = v;
        }
        if let Some(v) = f("zoom") {
            self.zoom = v.clamp(0.0, 1.0);
        }
        if let Some(v) = b("focus") {
            self.focus = v;
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
    pub(super) fn write_prefs(&self) {
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
             show_hidden={}\nwrap={}\ncol_size={}\ncol_date={}\npreview_visible={}\nsync_scroll={}\nzoom={}\n\
             view={}\ndetails={}\ncwd={}\nwindow={}\nfocus={}\n{pins}",
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
            self.sync_scroll,
            self.zoom,
            self.view.label(),
            self.details,
            self.cwd.to_string_lossy(),
            self.focus,
            self.window_rect
                .map(|[x, y, w, h]| format!("{x},{y},{w},{h}"))
                .unwrap_or_else(|| "-".to_owned()),
        );
        if let Err(e) = std::fs::write(&dir, body) {
            log::warn!("cannot write prefs: {e}");
        }
    }

    // ---- quick access ----------------------------------------------------

    /// The settings window: sections down the left, and the chosen one's settings as
    /// a list of rows on the right, each with what it does under its name and the
    /// control at the end of the row.
    pub(super) fn settings_ui(&mut self, ui: &mut Ui, section: &mut usize, close: &mut bool) {
        use egui_phosphor::regular as ph;
        const SIZE: Vec2 = Vec2::new(700.0, 460.0);
        const SIDE_W: f32 = 190.0;
        let (rect, _) = ui.allocate_exact_size(SIZE, Sense::hover());
        let side = Rect::from_min_size(rect.min, Vec2::new(SIDE_W, rect.height()));
        let main = Rect::from_min_max(Pos2::new(side.right(), rect.top()), rect.max);
        let painter = ui.painter().clone();
        painter.rect_filled(
            side,
            CornerRadius {
                nw: sp::RADIUS_LG,
                sw: sp::RADIUS_LG,
                ne: 0,
                se: 0,
            },
            c::PANEL,
        );
        painter.vline(side.right(), side.y_range(), Stroke::new(1.0, c::DIVIDER));

        // The sections.
        let title = widgets::layout(ui, "Settings".to_owned(), theme::bold_font(15.0), c::TEXT);
        widgets::galley_at(
            &painter,
            Pos2::new(side.left() + 20.0, side.top() + 22.0),
            &title,
            c::TEXT,
        );
        let sections = [
            ("General", ph::SLIDERS_HORIZONTAL),
            ("Appearance", ph::PALETTE),
            ("Editor", ph::CODE),
        ];
        for (i, (label, glyph)) in sections.iter().enumerate() {
            let row = Rect::from_min_size(
                Pos2::new(side.left() + 8.0, side.top() + 64.0 + i as f32 * 36.0),
                Vec2::new(SIDE_W - 16.0, 32.0),
            );
            let resp = ui.interact(row, Id::new(("settings-section", i)), Sense::click());
            if *section == i {
                painter.rect_filled(row, CornerRadius::same(6), c::SEL);
            } else if resp.hovered() {
                painter.rect_filled(row, CornerRadius::same(6), c::HOVER);
            }
            let color = if *section == i {
                c::SEL_TEXT
            } else {
                c::TEXT_DIM
            };
            Icon::Glyph(glyph).paint(
                &painter,
                Rect::from_center_size(
                    Pos2::new(row.left() + 20.0, row.center().y),
                    Vec2::splat(16.0),
                ),
                color,
            );
            let g = widgets::layout(ui, (*label).to_owned(), theme::ui_font(tfs::BODY), color);
            widgets::galley_at(
                &painter,
                Pos2::new(row.left() + 38.0, row.center().y - g.size().y * 0.5),
                &g,
                color,
            );
            if resp.clicked() {
                *section = i;
            }
        }

        // The header of the section, and the way out.
        let name = sections[(*section).min(sections.len() - 1)].0;
        let head = widgets::layout(ui, name.to_owned(), theme::bold_font(15.0), c::TEXT);
        widgets::galley_at(
            &painter,
            Pos2::new(main.left() + 28.0, main.top() + 22.0),
            &head,
            c::TEXT,
        );
        let x_rect = Rect::from_center_size(
            Pos2::new(main.right() - 26.0, main.top() + 30.0),
            Vec2::splat(28.0),
        );
        let x_resp = ui.interact(x_rect, Id::new("settings-close"), Sense::click());
        if x_resp.hovered() {
            painter.rect_filled(x_rect, CornerRadius::same(6), c::HOVER);
        }
        Icon::Close.paint(
            &painter,
            x_rect,
            if x_resp.hovered() {
                c::TEXT
            } else {
                c::TEXT_DIM
            },
        );
        if x_resp.on_hover_text("Close (Esc)").clicked() {
            *close = true;
        }

        // The rows.
        let body = Rect::from_min_max(
            Pos2::new(main.left() + 28.0, main.top() + 64.0),
            Pos2::new(main.right() - 28.0, main.bottom() - 16.0),
        );
        let mut rows = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        rows.spacing_mut().item_spacing = Vec2::ZERO;
        match *section {
            0 => {
                setting_row(
                    &mut rows,
                    "Show hidden files",
                    "List files and folders that are marked as hidden",
                    |ui| {
                        let mut on = self.show_hidden;
                        if widgets::switch(ui, &mut on).changed() {
                            self.show_hidden = on;
                            self.request_listing();
                        }
                    },
                );
                setting_row(
                    &mut rows,
                    "Search inside folders",
                    "A search looks in the folders below this one as well",
                    |ui| {
                        let mut on = self.scope == SearchScope::Below;
                        if widgets::switch(ui, &mut on).changed() {
                            self.scope = if on {
                                SearchScope::Below
                            } else {
                                SearchScope::Here
                            };
                            self.on_filter_changed();
                        }
                    },
                );
                setting_row(
                    &mut rows,
                    "Sidebar",
                    "The folder tree and Quick access at the left",
                    |ui| {
                        widgets::switch(ui, &mut self.sidebar);
                    },
                );
                setting_row(
                    &mut rows,
                    "Details pane",
                    "A preview and the properties of what is selected",
                    |ui| {
                        widgets::switch(ui, &mut self.details);
                    },
                );
            }
            1 => {
                setting_row(
                    &mut rows,
                    "Layout",
                    "How the files in a folder are listed",
                    |ui| {
                        let labels = ["Details", "List", "Icons"];
                        let at = ViewMode::ALL
                            .iter()
                            .position(|m| *m == self.view)
                            .unwrap_or(0);
                        if let Some(i) = widgets::segmented(ui, &labels, at) {
                            self.set_view(ViewMode::ALL[i]);
                        }
                    },
                );
                setting_row(
                    &mut rows,
                    "Item size",
                    "The size of icons, and how tall the rows are",
                    |ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(160.0, 24.0), Sense::hover());
                        let before = self.zoom;
                        widgets::size_slider(ui, r, &mut self.zoom);
                        if (self.zoom - before).abs() > f32::EPSILON {
                            self.row_cache.clear();
                        }
                    },
                );
            }
            _ => {
                setting_row(
                    &mut rows,
                    "Wrap long lines",
                    "Long lines continue on the next row instead of running off the side",
                    |ui| {
                        widgets::switch(ui, &mut self.wrap);
                    },
                );
                setting_row(
                    &mut rows,
                    "Markdown preview",
                    "Show the rendered page beside a Markdown file",
                    |ui| {
                        widgets::switch(ui, &mut self.preview_visible);
                    },
                );
                setting_row(
                    &mut rows,
                    "Lock scrolling",
                    "The editor and the preview scroll together",
                    |ui| {
                        let mut on = self.sync_scroll;
                        if widgets::switch(ui, &mut on).changed() {
                            self.sync_scroll = on;
                            if on {
                                self.align_preview_to_editor();
                            }
                        }
                    },
                );
            }
        }
    }

    // ---- dialogs -----------------------------------------------------------
}
