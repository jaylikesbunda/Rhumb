//! The title bar, the toolbar above the list, and the window edges.

use super::*;
use egui::{WidgetInfo, WidgetType};

impl Rhumb {
    /// The window is undecorated, so this bar owns moving, maximising and
    /// closing it, plus the resize edges along the bottom and right.
    pub(super) fn title_bar(&mut self, ui: &mut Ui) {
        let height = sp::TITLE;
        let buttons_w = sp::WIN_BTN_W * 3.0;
        let rect = Rect::from_min_size(ui.min_rect().min, Vec2::new(ui.available_width(), height));

        // App mark.
        let mark = widgets::layout(ui, "rhumb".to_owned(), theme::bold_font(11.5), c::TEXT_DIM);
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
        for (icon, action) in [
            (Icon::WinMinimize, WinAction::Minimize),
            (
                if maximized {
                    Icon::WinRestore
                } else {
                    Icon::WinMaximize
                },
                WinAction::Maximize,
            ),
            (Icon::WinClose, WinAction::Close),
        ] {
            let r = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(sp::WIN_BTN_W, height));
            let rresp = ui.interact(r, Id::new(("win", action as i32)), Sense::click());
            // The window buttons are glyphs, so each says what it does.
            let name = match action {
                WinAction::Minimize => "Minimize",
                WinAction::Maximize if maximized => "Restore",
                WinAction::Maximize => "Maximize",
                WinAction::Close => "Close window",
            };
            rresp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
            if ui.is_rect_visible(r) {
                let painter = ui.painter();
                let closing = action == WinAction::Close;
                if rresp.hovered() {
                    // Close goes red, as it does on Windows: it is the one button that
                    // discards something, and it should look like it.
                    painter.rect_filled(
                        r,
                        CornerRadius::ZERO,
                        if closing {
                            Color32::from_rgb(0xC4, 0x2B, 0x1C)
                        } else {
                            c::HOVER
                        },
                    );
                }
                let color = if rresp.hovered() && closing {
                    Color32::WHITE
                } else if rresp.hovered() {
                    c::TEXT
                } else {
                    c::TEXT_DIM
                };
                // The square glyphs fill more of their em than the minus and the
                // cross, so at the same size they read as larger.
                let glyph = match action {
                    WinAction::Maximize => 13.0,
                    _ => 16.0,
                };
                icon.paint_large(
                    painter,
                    Rect::from_center_size(r.center(), Vec2::splat(glyph)),
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
    pub(super) fn resize_edges(&mut self, ctx: &Context) {
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

    pub(super) fn window_action(&mut self, ctx: &Context, action: WinAction) {
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
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        let total = ui.available_width();
        // Reserve room for the fixed clusters, then give the address bar what is
        // left. Below the floor, the address bar collapses to a Go button.
        let controls_w = 358.0f32; // New, sort, filter, hidden files, details, dual, settings
        let search_w = (total * 0.24).clamp(110.0, 240.0);
        // Sidebar toggle, back, forward, up, and refresh.
        let nav_w = 5.0 * 26.0 + 3.0 * 2.0;
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
            ui.add_space(sp::XS);
            if widgets::icon_button(ui, Icon::Refresh, "Refresh (F5)").clicked() {
                self.request_listing();
            }
            ui.add_space(sp::SM);

            self.search_box(ui, search_w);
            ui.add_space(sp::SM);
            self.new_button(ui);
            self.sort_button(ui);
            self.filter_button(ui);
            self.hidden_toggle(ui);
            self.details_toggle(ui);
            self.dual_toggle(ui);
            if widgets::icon_button(
                ui,
                Icon::Glyph(egui_phosphor::regular::GEAR_SIX),
                "Settings (Ctrl+,)",
            )
            .clicked()
            {
                self.dialog = Dialog::Settings { section: 0 };
            }
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
    pub(super) fn new_button(&mut self, ui: &mut Ui) {
        // U+25BC, not U+25BE: Inter has no small triangle and neither does
        // egui's fallback, so the latter renders as a tofu box. Pinned by
        // `every_ui_symbol_is_in_the_font`.
        let resp = widgets::flat_button(ui, "New \u{25BC}", "Create something (Alt+F)");
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
    pub(super) fn address_bar(&mut self, ui: &mut Ui, width: f32) {
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

    /// Sort column and direction, plus how the list is grouped.
    pub(super) fn sort_button(&mut self, ui: &mut Ui) {
        let mut action = None;
        let mut group_action = None;
        let resp = compact_button(
            ui,
            &format!(
                "Sort: {} {}",
                self.sort.label(),
                if self.ascending {
                    "\u{2191}"
                } else {
                    "\u{2193}"
                }
            ),
            "Sort order and grouping (click to change)",
        );
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Sort order"));
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
            ui.separator();
            widgets::menu_heading(ui, "Group by");
            for g in GroupBy::ALL {
                if ui.selectable_label(self.group_by == g, g.label()).clicked() {
                    group_action = Some(g);
                }
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
        if let Some(g) = group_action
            && self.group_by != g
        {
            self.group_by = g;
            self.recompute_visible();
        }
    }

    /// The filter menu: kind, modified date and size, which combine with each
    /// other and with the name box. Lit while any of them is narrowing the list.
    pub(super) fn filter_button(&mut self, ui: &mut Ui) {
        use egui_phosphor::regular as ph;
        let active = self.filters_active();
        let tip = if active {
            "Filters are on (click to change)"
        } else {
            "Filter by kind, date or size"
        };
        let resp = widgets::icon_toggle(ui, Icon::Glyph(ph::FUNNEL), active, tip);
        let mut kind = None;
        let mut date = None;
        let mut size = None;
        let mut clear = false;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_width(210.0);
            ui.spacing_mut().item_spacing = Vec2::new(sp::SM, 2.0);
            widgets::menu_heading(ui, "Kind");
            for k in KindFilter::ALL {
                if ui
                    .selectable_label(self.kind_filter == k, k.label())
                    .clicked()
                {
                    kind = Some(k);
                }
            }
            ui.add_space(sp::SM);
            widgets::menu_heading(ui, "Modified");
            for d in DateFilter::ALL {
                if ui
                    .selectable_label(self.date_filter == d, d.label())
                    .clicked()
                {
                    date = Some(d);
                }
            }
            ui.add_space(sp::SM);
            widgets::menu_heading(ui, "Size");
            for s in SizeFilter::ALL {
                if ui
                    .selectable_label(self.size_filter == s, s.label())
                    .clicked()
                {
                    size = Some(s);
                }
            }
            if active {
                ui.separator();
                if ui.button("Clear filters").clicked() {
                    clear = true;
                }
            }
        });
        if clear {
            self.clear_filters();
            return;
        }
        let mut changed = false;
        if let Some(k) = kind {
            self.kind_filter = k;
            changed = true;
        }
        if let Some(d) = date {
            self.date_filter = d;
            changed = true;
        }
        if let Some(s) = size {
            self.size_filter = s;
            changed = true;
        }
        if changed {
            self.recompute_visible();
        }
    }

    /// Whether hidden files are listed: an eye that is open when they are. One
    /// control for one thing, which is on or off, so it is a switch and not a menu.
    pub(super) fn hidden_toggle(&mut self, ui: &mut Ui) {
        let on = self.show_hidden;
        let tip = if on {
            "Hidden files are shown (Ctrl+H)"
        } else {
            "Hidden files are not shown (Ctrl+H)"
        };
        if widgets::icon_toggle(ui, if on { Icon::Eye } else { Icon::EyeSlash }, on, tip).clicked()
        {
            self.show_hidden = !self.show_hidden;
            self.request_listing();
        }
    }

    /// The file list's right-hand pane, on or off. Kept in the toolbar so a pane
    /// closed from its own header always has a control in plain sight to bring it
    /// back, even on a window too narrow for the status bar's own switch.
    pub(super) fn details_toggle(&mut self, ui: &mut Ui) {
        let on = self.details;
        let tip = if on {
            "Details pane is shown (Alt+P)"
        } else {
            "Details pane is hidden (Alt+P)"
        };
        let icon = Icon::Glyph(egui_phosphor::regular::SQUARE_HALF);
        if widgets::icon_toggle(ui, icon, on, tip).clicked() {
            self.details = !self.details;
            // Turning it on with nothing selected would show an empty pane and look
            // like nothing happened, so the first row is selected to give it a subject.
            if self.details
                && self.sel.is_empty()
                && !self.visible.is_empty()
                && let Some(p) = self.path_at(0)
            {
                self.sel.insert(p);
                self.cursor = 0;
                self.anchor = 0;
            }
        }
    }

    /// The dual-pane view, on or off: two folder lists side by side.
    pub(super) fn dual_toggle(&mut self, ui: &mut Ui) {
        let on = self.dual;
        let tip = if on {
            "Two folder lists are shown (Ctrl+Shift+D)"
        } else {
            "Show two folder lists (Ctrl+Shift+D)"
        };
        let icon = Icon::Glyph(egui_phosphor::regular::COLUMNS);
        if widgets::icon_toggle(ui, icon, on, tip).clicked() {
            self.toggle_dual();
        }
    }

    pub(super) fn search_box(&mut self, ui: &mut Ui, width: f32) {
        let mut text = self.filter.clone();
        let running = self.search.running;
        let searched = self.search_shown && !self.search.results.is_empty();

        // The inner response is the text field's, not the frame's: reading
        // `.response` off the frame instead meant typing never reached the
        // filter and the box reset on every frame.
        let place = self.cwd.file_name().map_or_else(
            || "this drive".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let scope = self.scope;
        let mut flip_scope = false;
        let mut clear = false;
        // Room for the clear button, so the field shrinks rather than the box growing.
        let clear_w = if text.is_empty() { 0.0 } else { 20.0 };
        let out = Frame::new()
            .fill(c::CODE_BG)
            .stroke(Stroke::new(1.0, c::BORDER))
            .corner_radius(CornerRadius::same(sp::RADIUS))
            .inner_margin(Margin::symmetric(sp::SM_I, 4))
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
                let hint = match (searched, scope) {
                    (true, _) => format!("{} matches", self.search.results.len()),
                    (false, SearchScope::Below) => format!("Search {place}"),
                    (false, SearchScope::Here) => format!("Filter {place}"),
                };
                let field = TextEdit::singleline(&mut text)
                    .id(Id::new(ID_SEARCH))
                    .hint_text(hint)
                    .frame(Frame::NONE)
                    .desired_width((width - 34.0 - 26.0 - clear_w).max(40.0))
                    .text_color(c::TEXT)
                    .show(ui)
                    .response;
                // Clear the box and keep the caret in it, so a new search can be typed
                // at once. Only there while there is something to clear.
                if !text.is_empty() {
                    let (crect, cresp) =
                        ui.allocate_exact_size(Vec2::new(18.0, 18.0), Sense::click());
                    cresp.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::Button, true, "Clear search")
                    });
                    if cresp.hovered() {
                        ui.painter()
                            .rect_filled(crect, CornerRadius::same(4), c::HOVER);
                    }
                    Icon::Close.paint(
                        ui.painter(),
                        crect,
                        if cresp.hovered() { c::TEXT } else { c::TEXT_FAINT },
                    );
                    if cresp.on_hover_text("Clear").clicked() {
                        clear = true;
                    }
                }
                // Whether the search goes into the folders below or stays in this one:
                // a button in the box it belongs to, lit while it goes in.
                let deep = scope == SearchScope::Below;
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(22.0, 18.0), Sense::click());
                resp.widget_info(|| {
                    WidgetInfo::selected(WidgetType::Button, true, deep, "Search subfolders")
                });
                if deep {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(4), c::SEL);
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(4), c::HOVER);
                }
                Icon::Subfolders.paint(
                    ui.painter(),
                    rect,
                    if deep {
                        c::ACCENT
                    } else if resp.hovered() {
                        c::TEXT
                    } else {
                        c::TEXT_FAINT
                    },
                );
                if resp
                    .on_hover_text(if deep {
                        "Searching this folder and the folders inside it (click to search only this folder)"
                    } else {
                        "Searching this folder only (click to include the folders inside it)"
                    })
                    .clicked()
                {
                    flip_scope = true;
                }
                field
            })
            .inner;
        if flip_scope {
            self.scope = match self.scope {
                SearchScope::Below => SearchScope::Here,
                SearchScope::Here => SearchScope::Below,
            };
            self.on_filter_changed();
        }

        if clear {
            self.filter.clear();
            self.on_filter_changed();
            self.search_focus = true;
        } else if out.changed() {
            self.filter = text;
            self.on_filter_changed();
        }
        if self.search_focus {
            out.request_focus();
            self.search_focus = false;
        }
    }

    // ---- sidebar ----------------------------------------------------------

    // ---- sidebar ----------------------------------------------------------
}
