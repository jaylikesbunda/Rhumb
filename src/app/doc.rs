//! The open files: the tab strip, the editor and preview panes, and saving and closing.

use super::*;

impl Rhumb {
    /// The tab on screen.
    pub(super) fn tab(&self) -> Option<&Tab> {
        self.tabs.active_tab()
    }

    pub(super) fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.active_tab_mut()
    }

    /// The document on screen, if any.
    pub(super) fn doc(&self) -> Option<&Doc> {
        self.tab().map(|t| &t.doc)
    }

    pub(super) fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.tab_mut().map(|t| &mut t.doc)
    }

    pub(super) fn has_tabs(&self) -> bool {
        !self.tabs.is_empty()
    }

    /// Whether there is a file open, which is what puts the editor up.
    pub(super) fn shows_file_tab(&self) -> bool {
        !self.tabs.is_empty()
    }

    /// Index of the tab showing `path`, if it is already open.
    pub(super) fn tab_index(&self, path: &Path) -> Option<usize> {
        self.tabs.index_of(path)
    }

    /// Brings a tab to the front, rebuilding the preview cache if it changed.
    /// The document on screen is now a different one.
    ///
    /// Everything cached against the old document goes: the rendered preview,
    /// its version stamp, the pending refresh, and the editor's own state. The
    /// caret, the scroll position, the selection and the undo history all
    /// describe a file that is no longer there, and a stale line index would
    /// quietly mis-lay-out the new one.
    pub(super) fn doc_changed(&mut self) {
        self.preview.reset();
        self.render_version = 0;
        // The text last handed to the preview belonged to the file that was on screen,
        // and a new file starts at the same version number as the old one did, so the
        // stamp that says "nothing new to copy" has to go too.
        self.preview_buffer_version = u64::MAX;
        self.last_edit = None;
        self.ed.reset();
        self.ed_focused = false;
    }

    /// Brings one of the open files forward.
    pub(super) fn focus_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if index == self.tabs.active {
            return;
        }
        self.tabs.focus(index);
        self.doc_changed();
    }

    /// Ctrl+Tab: the next tab. In the editor that is the next file; anywhere else it is the
    /// next folder tab.
    pub(super) fn cycle_tab(&mut self, back: bool) {
        if self.ed_focused && self.tabs.len() > 1 {
            self.cycle_file(back);
        } else {
            self.cycle_folder(back);
        }
    }

    /// The next open file, or the one before, wrapping round.
    pub(super) fn cycle_file(&mut self, back: bool) {
        let n = self.tabs.len();
        if n < 2 {
            return;
        }
        let before = self.tabs.active;
        let next = if back {
            (before + n - 1) % n
        } else {
            (before + 1) % n
        };
        self.focus_tab(next);
    }

    /// Closes one open file, moving focus to a neighbour.
    pub(super) fn close_tab(&mut self, index: usize) {
        self.tabs.close(index);
        self.doc_changed();
    }

    // ---- frame ---------------------------------------------------------

    /// The editor pane: the row of open files, and the one that is in front under it.
    pub(super) fn doc_ui(&mut self, ui: &mut Ui) {
        if !self.tabs.is_empty() {
            let (strip, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), sp::TAB_H), Sense::hover());
            self.doc_strip_ui(ui, strip);
            // The rest of the pane is laid out from where the row ends, and not from the
            // top of the pane, which is where the code below measures from.
            let rest =
                Rect::from_min_max(Pos2::new(strip.left(), strip.bottom()), ui.max_rect().max);
            let mut inner = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rest)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            self.doc_body(&mut inner);
            return;
        }
        self.doc_body(ui);
    }

    /// The file in front: its header, and the editor or editor and preview.
    fn doc_body(&mut self, ui: &mut Ui) {
        if self.loading.is_some() {
            widgets::empty_state(ui, "Loading\u{2026}", "Reading the file");
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
        let header_h = HEADER_TITLE_H + HEADER_PATH_H;
        let header =
            Rect::from_min_size(ui.min_rect().min, Vec2::new(ui.available_width(), header_h));
        let title_row = Rect::from_min_max(
            header.min,
            Pos2::new(header.right(), header.top() + HEADER_TITLE_H),
        );

        // Actions first, so the title knows how much room is left.
        let mut save = false;
        let mut wrap = false;
        let mut preview = false;
        let mut sync = false;
        let mut close = false;
        let mut focus_toggle = false;
        let focused = self.focus;
        let btn = 26.0f32;
        let linkable = is_md && self.preview_visible;
        let action_count = 4 + usize::from(is_md) + usize::from(linkable) + usize::from(dirty);
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
            // Focus fills the window with the file; the same button brings
            // the list back. Next to Close, where window controls live.
            let (fr, frr) = bar.allocate_exact_size(Vec2::splat(btn), Sense::click());
            if frr.hovered() {
                bar.painter()
                    .rect_filled(fr, CornerRadius::same(sp::RADIUS), c::HOVER);
            }
            (if focused { Icon::Shrink } else { Icon::Expand }).paint(
                bar.painter(),
                fr,
                if frr.hovered() { c::TEXT } else { c::TEXT_DIM },
            );
            if frr
                .on_hover_text(if focused {
                    "Show the file list (F11)"
                } else {
                    "Fill the window with the file (F11)"
                })
                .clicked()
            {
                focus_toggle = true;
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
            if linkable {
                let (r, rr) = bar.allocate_exact_size(Vec2::splat(btn), Sense::click());
                if rr.hovered() {
                    bar.painter()
                        .rect_filled(r, CornerRadius::same(sp::RADIUS), c::HOVER);
                }
                (if self.sync_scroll {
                    Icon::Link
                } else {
                    Icon::LinkOff
                })
                .paint(
                    bar.painter(),
                    r,
                    if self.sync_scroll {
                        c::ACCENT
                    } else if rr.hovered() {
                        c::TEXT
                    } else {
                        c::TEXT_DIM
                    },
                );
                if rr
                    .on_hover_text(if self.sync_scroll {
                        "Scrolling is locked: the editor and preview move together (click to unlock)"
                    } else {
                        "Lock scrolling so the editor and preview move together"
                    })
                    .clicked()
                {
                    sync = true;
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
            if wrr
                .on_hover_text("Soft wrap long lines (hides line numbers)")
                .clicked()
            {
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

        panel_header(
            ui,
            header,
            title_row,
            if is_md { Icon::Markdown } else { Icon::File },
            file_name.clone(),
            location,
            dirty,
            read_only,
            actions_w,
        );
        if preview {
            self.preview_visible = !self.preview_visible;
        }
        if sync {
            self.sync_scroll = !self.sync_scroll;
            if self.sync_scroll {
                // Locking brings the preview to where the editor is, so the lock starts from
                // the two agreeing rather than from whichever one moved last.
                self.align_preview_to_editor();
            }
        }
        if focus_toggle {
            self.focus = !self.focus;
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
                // The text under the caret just changed, so the caret may now
                // point past the end of it.
                self.doc_changed();
            }
            if keep && let Some(d) = self.doc_mut() {
                d.externally_changed = false;
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
    pub(super) fn split_ui(&mut self, ui: &mut Ui, rect: Rect) {
        let divider_w = 7.0f32;
        let usable = (rect.width() - divider_w).max(1.0);
        let left_w = split_left(usable, self.split);
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
            // Drag in pixels and convert back through the same clamp, so
            // grabbing a divider that is already against its limit moves it
            // smoothly instead of snapping somewhere else.
            self.split = split_fraction(usable, left_w + resp.drag_delta().x);
        }

        let mut right_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right)
                .layout(egui::Layout::top_down(egui::Align::LEFT))
                .id_salt("preview-pane"),
        );
        self.preview_ui(&mut right_ui, right);
        if self.sync_scroll {
            self.sync_panes(ui);
        }
    }

    /// Keeps the editor and the preview at the same place in the file, whichever of
    /// them is being scrolled. A place is a line of the source: the preview knows which
    /// line each of its blocks starts on, so it can say what line it is showing and be
    /// put at a given one. That holds still while the preview learns how tall its blocks
    /// really are, which a fraction of the way down does not.
    pub(super) fn sync_panes(&mut self, ui: &Ui) {
        let ed = self.ed.scroll_line();
        // The preview's own report of where it is runs behind while it is still making
        // up for blocks that turned out taller or shorter than expected, so that is
        // added in: read raw, it looked like the preview had been scrolled.
        let off = self.preview_off + self.preview.pending_shift();
        let pv = self.preview.line_at_y(off);
        let ed_moved = (ed - self.sync_ed).abs() > 0.02;
        let pv_moved = (pv - self.sync_pv).abs() > 0.02;
        let over_preview = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| self.preview_rect.contains(p));
        if pv_moved && (over_preview || !ed_moved) {
            // The preview is the one being scrolled: the editor follows it. At the very
            // bottom of the preview the editor goes to its own bottom, since the two
            // ends do not line up by lines.
            self.editor_leads = false;
            self.follow_preview(off, pv);
            ui.ctx().request_repaint();
        } else if ed_moved {
            self.editor_leads = true;
            self.align_preview_to_editor();
            ui.ctx().request_repaint();
        } else if self.panes_apart(off, ed, pv) {
            // Neither was touched, but learning the real heights of blocks has moved the
            // two apart: the one that was not scrolled last goes back to the other.
            if self.editor_leads {
                self.align_preview_to_editor();
            } else {
                self.follow_preview(off, pv);
            }
            ui.ctx().request_repaint();
        }
    }

    /// Whether the editor and the preview are showing places that are not the same.
    /// Both at the top, or both at the bottom, are the same place whatever their lines.
    fn panes_apart(&self, off: f32, ed: f32, pv: f32) -> bool {
        let ed_end = self.ed.scroll_fraction() >= 0.999;
        let pv_end = self.preview_range > 0.0 && off >= self.preview_range - 2.0;
        if ed_end && pv_end {
            return false;
        }
        if ed_end != pv_end && (ed_end || pv_end) {
            return true;
        }
        (ed - pv).abs() > 1.0
    }

    /// Takes the editor to where the preview is.
    fn follow_preview(&mut self, off: f32, pv: f32) {
        if self.preview_range > 0.0 && off >= self.preview_range - 1.0 {
            self.ed.set_scroll_fraction(1.0);
        } else {
            self.ed.set_scroll_line(pv);
        }
        self.sync_pv = pv;
        self.sync_ed = self.ed.scroll_line();
    }

    /// Puts the preview where the editor is.
    pub(super) fn align_preview_to_editor(&mut self) {
        let ed = self.ed.scroll_line();
        let y = if self.ed.scroll_fraction() >= 0.999 {
            self.preview_range
        } else {
            self.preview.y_of_line(ed).min(self.preview_range)
        };
        self.preview_set = Some(y);
        self.sync_ed = ed;
        self.sync_pv = self.preview.line_at_y(y);
    }

    /// The code editor.
    pub(super) fn editor_ui(&mut self, ui: &mut Ui, rect: Rect) {
        // The wrapping toggle gives up the gutter, because a wrapped line has no
        // single row to put a number against. The toggle in the header says so.
        let Some(tab) = self.tabs.active_tab() else {
            return;
        };
        let read_only = tab.doc.read_only;
        let wrap = self.wrap;
        let opts = codeedit::Options {
            // Always on, including while wrapping. A wrapped line has no single
            // row to put a number against, which used to be the reason for
            // dropping the gutter entirely — but the number belongs to the *line*,
            // and every editor that wraps still shows it against the first row of
            // the line. Losing the gutter is a far bigger loss than the small
            // irregularity of one number per line rather than per row.
            line_numbers: true,
            wrap,
            // Only for files whose language this editor recognises. The keyword
            // list is shared across every language it covers, so running it over
            // a plain text file lights up every ordinary English word that
            // happens to be a keyword somewhere, which looks broken rather than
            // helpful.
            highlight: editing::highlights_code(&tab.doc.path),
            editable: !read_only,
            comment: editing::comment_token(&tab.doc.path),
            lang: editing::lang_for(&tab.doc.path),
        };
        let bench = bench_on();
        // Borrowed apart by hand rather than through `doc_mut`, because the
        // widget needs the editor's state and the buffer at the same time, and
        // a method call would borrow all of `self` for both.
        let Self {
            ed,
            tabs,
            last_edit,
            ..
        } = self;
        let t0 = Instant::now();
        let tab = tabs.active_tab_mut().expect("checked above");
        let out = ed.show(ui, rect, &mut tab.doc.text, &opts);
        let focused = ed.focused();
        if out.edited {
            // The buffer changed under us, so the Markdown preview is stale. It is
            // not brought up to date here: it catches up once typing pauses (see
            // `preview_ui`). Doing it on every keystroke made each one pay for parsing
            // the whole document, which grows with the size of the file.
            tab.doc.touch();
            *last_edit = Some(Instant::now());
        }
        let dt = t0.elapsed().as_secs_f32() * 1000.0;
        let bytes = tab.doc.text.len_bytes();
        self.ed_focused = focused;
        self.ed_took_clipboard = out.clipboard;
        if bench {
            self.report_editor_bench(dt, bytes);
        }
    }

    /// Records how long the editor took, and logs the worst frame seen so far.
    ///
    /// Only when `RHUMB_BENCH` is set, and the whole probe is two comparisons
    /// and an add otherwise. It reports a running maximum rather than logging
    /// every slow frame, because a frame that is fast is exactly as interesting
    /// as one that is not, and a log full of nothing is easy to mistake for a
    /// broken probe.
    pub(super) fn report_editor_bench(&mut self, ms: f32, bytes: usize) {
        self.bench_frames += 1;
        self.bench_total += ms;
        self.bench_max = self.bench_max.max(ms);
        let due = self
            .bench_since
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
        if !due {
            return;
        }
        let frames = self.bench_frames.max(1);
        log::info!(
            "BENCH editor worst {:.2} ms, {:.2} ms mean over {frames} frames, {bytes} bytes, \
             whole frame {:.2} ms of which housekeeping {:.2} ms; worst by part: {}",
            self.bench_max,
            self.bench_total / frames as f32,
            self.perf.update_ms,
            self.bench_house,
            self.bench_sections
                .iter()
                .map(|(n, ms)| format!("{n} {ms:.2}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        self.bench_sections.clear();
        self.bench_since = Some(Instant::now());
        self.bench_frames = 0;
        self.bench_total = 0.0;
        self.bench_max = 0.0;
    }

    /// Markdown preview, refreshed after a short pause in typing.
    pub(super) fn preview_ui(&mut self, ui: &mut Ui, _rect: Rect) {
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
        // Copying the document every frame is a full memcpy of the buffer, and
        // the preview is idle on almost all of them: the version only moves
        // when there is something new to render.
        if version != self.preview_buffer_version {
            self.preview_buffer = self.doc().map(|d| d.text.to_text()).unwrap_or_default();
            self.preview_buffer_version = version;
        }
        self.preview
            .sync_in_background(ui.ctx(), &self.preview_buffer, version);

        let indent = sp::LG;
        // The `Ui` we were handed is already anchored to `rect`, so the scroll
        // area simply fills it.
        let mut area = egui::ScrollArea::vertical()
            .id_salt("preview")
            .animated(false)
            .auto_shrink([false, false]);
        if let Some(y) = self.preview_set.take() {
            area = area.vertical_scroll_offset(y);
        }
        let out = area.show(ui, |ui| {
            let height = self.preview.show(ui, indent);
            ui.allocate_exact_size(
                Vec2::new(ui.available_width(), height + sp::XL),
                Sense::hover(),
            );
        });
        self.preview_off = out.state.offset.y;
        self.preview_range = (out.content_size.y - out.inner_rect.height()).max(0.0);
        self.preview_rect = out.inner_rect;
    }

    // ---- status bar --------------------------------------------------------

    pub(super) fn request_close(&mut self, ctx: &Context) {
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

    pub(super) fn close_guard(&mut self, ctx: &Context) {
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

    /// The first file with unsaved changes, for the "save before closing" prompt. One in
    /// another folder tab is found too, and that tab is brought forward so it can be seen.
    pub(super) fn unsaved_tab(&mut self) -> Option<usize> {
        if let Some(i) = self.tabs.first_dirty() {
            return Some(i);
        }
        let other = (0..self.folders.len()).find(|i| self.folder_has_unsaved(*i))?;
        self.switch_folder(other);
        self.tabs.first_dirty()
    }

    /// Closes the file on screen, used by `Ctrl+W` and the tab strip.
    pub(super) fn close_active_tab(&mut self) {
        if self.has_tabs() {
            let active = self.tabs.active;
            self.close_tab(active);
        }
    }

    /// The row of open files above the editor: each is a tab, pressing one brings it
    /// forward, the cross and the middle button close it, and they can be dragged into
    /// another order.
    pub(super) fn doc_strip_ui(&mut self, ui: &mut Ui, rect: Rect) {
        let items: Vec<strip::Item> = self
            .tabs
            .iter()
            .map(|t| strip::Item {
                id: t.id,
                label: t.label(),
                icon: if t.doc.kind == DocKind::Markdown {
                    Icon::Markdown
                } else {
                    Icon::File
                },
                dirty: t.doc.dirty(),
            })
            .collect();
        let action = strip::show(
            ui,
            rect,
            &items,
            self.tabs.active,
            &mut self.doc_strip,
            &strip::Style {
                salt: "file-tabs",
                new_tip: None,
                min_w: 96.0,
                max_w: 190.0,
            },
        );
        if let Some((from, to)) = action.reorder {
            self.tabs.move_tab(from, to);
        }
        if let Some(i) = action.focus.or(action.pick) {
            self.focus_tab(i);
        }
        if let Some(i) = action.close
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

    /// Opens a file in a tab, or focuses the tab that already has it.
    pub(super) fn open_path(&mut self, path: &Path) {
        if path.is_dir() {
            self.navigate(path);
            return;
        }
        // An archive opens as a folder, and so does a folder inside one.
        if archive::is_archive_file(path) {
            self.navigate(path);
            return;
        }
        if let Some(inside) = archive::split(path)
            && archive::is_dir_inside(&inside.archive, &inside.inner)
        {
            self.navigate(path);
            return;
        }
        // Already open: just bring it forward, the way Explorer does.
        if let Some(i) = self.tab_index(path) {
            self.focus_tab(i);
            return;
        }
        // Any file opens. The extension is not a gate: a `.xyz` config is text
        // more often than not, and a file that turns out not to be text comes
        // back from the reader read-only with a warning rather than being
        // refused. Anything too large to hold is handled the same way.

        // A tab of its own among the files open in this folder tab.
        if self.tabs.len() >= MAX_TABS {
            self.toast(format!("At most {MAX_TABS} files open: close one first"));
            return;
        }
        let i = self.tabs.push(Tab::file(path));
        self.tabs.active = i;
        self.doc_changed();

        // Read on a worker thread so a large file never stalls the UI. The tab
        // exists already, so the strip shows it while the text arrives.
        self.loading = Some(path.to_path_buf());
        let tx = self.tx.clone();
        let read_path = path.to_path_buf();
        let watch_dir = path.parent().map(|p| p.to_path_buf());
        let _ = std::thread::Builder::new()
            .name("rhumb-read".into())
            .spawn(move || {
                // A file inside an archive is brought out to a real one first, and
                // read from there, but it stays named for where it came from and
                // cannot be saved: nothing is ever written back into an archive.
                if archive::is_virtual(&read_path) {
                    let msg = match archive::materialize(&read_path) {
                        Ok(real) => {
                            let (_, doc, error) = Doc::read(&real);
                            Msg::Loaded {
                                path: read_path.clone(),
                                doc: doc.map(|mut d| {
                                    d.path = read_path.clone();
                                    d.read_only = true;
                                    d
                                }),
                                error,
                            }
                        }
                        Err(e) => Msg::Loaded {
                            path: read_path.clone(),
                            doc: None,
                            error: Some(e.to_string()),
                        },
                    };
                    let _ = tx.send(msg);
                    return;
                }
                let (path, doc, error) = Doc::read(&read_path);
                let _ = tx.send(Msg::Loaded { path, doc, error });
            });
        if let Some(dir) = watch_dir
            && !archive::is_virtual(&dir)
        {
            self.watch(&dir);
        }
    }

    pub(super) fn close_doc(&mut self) {
        // With no file open, Ctrl+W closes the folder tab instead, as it does in a browser.
        if !self.has_tabs() {
            let active = self.active_folder;
            self.close_folder_tab(active);
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

    pub(super) fn save_doc(&mut self) {
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
}
