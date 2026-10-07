//! The status bar and the toasts.

use super::*;
use egui::{WidgetInfo, WidgetType};

impl Rhumb {
    pub(super) fn status_ui(&mut self, ui: &mut Ui) {
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
            if searching && self.search.indexed {
                format!("from an index of {} names", self.search.scanned)
            } else if searching && self.search.scanned > 0 {
                format!("{} scanned", self.search.scanned)
            } else if let Some(ix) = self.indexes.any_for(&self.cwd)
                && !ix.is_ready()
            {
                format!("Indexing\u{2026} {}", ix.scanned())
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

        // A running job gets a pause switch and a cancel affordance in the bar.
        let mut cancel_job = None;
        let mut toggle_pause = None;
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
                    if let Some(j) = self.jobs.first()
                        && j.pausable()
                    {
                        let (icon, tip) = if j.paused {
                            (Icon::Glyph(egui_phosphor::regular::PLAY), "Resume")
                        } else {
                            (Icon::Glyph(egui_phosphor::regular::PAUSE), "Pause")
                        };
                        if widgets::icon_button(ui, icon, tip).clicked() {
                            toggle_pause = Some(j.job.id);
                        }
                    }
                });
            });
        }
        if let Some(id) = toggle_pause
            && let Some(j) = self.jobs.iter_mut().find(|j| j.job.id == id)
        {
            j.paused = !j.paused;
            j.job
                .paused
                .store(j.paused, std::sync::atomic::Ordering::Relaxed);
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
        // The controls take the right-hand end, and the text keeps to their left.
        let controls = rect.width() > 560.0;
        if controls {
            right_x -= STATUS_CONTROLS_W;
        }
        if let Some(j) = self.jobs.first() {
            // What is being worked on now, plus the rate and time left once
            // there is enough of both to say.
            let mut text = j.current.clone();
            let rate = ops::rate_eta_text(j.done_bytes, j.total_bytes, j.job.started.elapsed());
            if !rate.is_empty() {
                if !text.is_empty() {
                    text.push_str("  \u{00B7}  ");
                }
                text.push_str(&rate);
            }
            let g =
                widgets::layout_elided(ui, text, theme::ui_font(tfs::SMALL), c::TEXT_GHOST, 220.0);
            widgets::text_right(
                painter,
                Pos2::new(right_x, rect.center().y),
                &g,
                c::TEXT_GHOST,
            );
            right_x -= 228.0;
        }
        // Where the caret is, and how big the file is. The line and column come
        // first because they are the two numbers a reader is actually looking for
        // while editing, and the size and timings after because they are what a
        // reader looks for when nothing is being edited.
        let info = match self.doc() {
            Some(d) => {
                let stats = self.preview.stats();
                let kb = d.text.len_bytes() as f32 / 1024.0;
                let (line, col) = self.ed.line_and_column(&d.text);
                // The editor's own count, not a fresh one: this runs every frame,
                // and counting the characters of a multi-megabyte file here is a
                // pass over the whole of it for a number the editor already has.
                let selection = match self.ed.selection(self.ed.len()) {
                    (lo, hi) if lo != hi => {
                        let n = hi - lo;
                        format!("  ({n} selected)")
                    }
                    _ => String::new(),
                };
                // More than one caret says how many.
                let carets = match self.ed.cursor_count() {
                    1 => String::new(),
                    n => format!("  ({n} carets)"),
                };
                let selection = format!("{selection}{carets}");
                // The parse time is only of interest to a reader of a Markdown
                // file, and there is only room for it if the other three are not
                // there. So it appears when it is worth something and vanishes
                // when it is not, rather than sitting at "0 us" taking the width
                // that would have shown the frame time.
                let parse = match stats.parse_us {
                    0 => String::new(),
                    us => format!("  \u{00B7}  {us} us parse"),
                };
                format!(
                    "Ln {line}, Col {col}{selection}  \u{00B7}  {kb:.1} KB{parse}  \
                     \u{00B7}  {:.1} ms",
                    self.perf.update_ms
                )
            }
            None => format!("{:.1} ms", self.perf.update_ms),
        };
        let g = widgets::layout_elided(ui, info, theme::ui_font(tfs::SMALL), c::TEXT_GHOST, 320.0);
        widgets::text_right(
            painter,
            Pos2::new(right_x, rect.center().y),
            &g,
            c::TEXT_GHOST,
        );

        // At the far right, the size of things and then how they are laid out, the
        // two controls that belong together. Three pictograms rather than words.
        let seg_w = 26.0f32;
        let segs = ViewMode::ALL.len() as f32 * seg_w;
        let segs_rect = Rect::from_min_max(
            Pos2::new(rect.right() - sp::SM - segs, rect.top() + 3.0),
            Pos2::new(rect.right() - sp::SM, rect.bottom() - 3.0),
        );
        if controls {
            let slider_rect = Rect::from_min_max(
                Pos2::new(segs_rect.left() - 14.0 - 92.0, rect.top()),
                Pos2::new(segs_rect.left() - 14.0, rect.bottom()),
            );
            let before = self.zoom;
            widgets::size_slider(ui, slider_rect, &mut self.zoom);
            if (self.zoom - before).abs() > f32::EPSILON {
                self.row_cache.clear();
            }
            // The details pane's switch, to the left of the size slider, so the pane
            // closed from its own header has a way back that is in plain sight.
            let toggle = Rect::from_min_max(
                Pos2::new(slider_rect.left() - 10.0 - seg_w, segs_rect.top()),
                Pos2::new(slider_rect.left() - 10.0, segs_rect.bottom()),
            );
            let resp = ui.interact(toggle, Id::new("details-toggle"), Sense::click());
            resp.widget_info(|| {
                WidgetInfo::selected(WidgetType::Button, true, self.details, "Details pane")
            });
            if self.details || resp.hovered() {
                ui.painter().rect_filled(
                    toggle,
                    CornerRadius::same(4),
                    if self.details { c::SEL } else { c::HOVER },
                );
            }
            Icon::Glyph(egui_phosphor::regular::SQUARE_HALF).paint(
                ui.painter(),
                toggle,
                if self.details {
                    c::SEL_TEXT
                } else {
                    c::TEXT_DIM
                },
            );
            if resp.on_hover_text("Details pane (Alt+P)").clicked() {
                self.details = !self.details;
            }
            let mut picked = None;
            for (i, mode) in ViewMode::ALL.iter().enumerate() {
                let seg = Rect::from_min_size(
                    Pos2::new(segs_rect.left() + i as f32 * seg_w, segs_rect.top()),
                    Vec2::new(seg_w, segs_rect.height()),
                );
                let resp = ui.interact(seg, Id::new(("view", mode.label())), Sense::click());
                let active = *mode == self.view;
                // The three view buttons are one selectable control each.
                resp.widget_info(|| {
                    WidgetInfo::selected(WidgetType::Button, true, active, mode.label())
                });
                if ui.is_rect_visible(seg) {
                    if active || resp.hovered() {
                        ui.painter().rect_filled(
                            seg,
                            CornerRadius::same(4),
                            if active { c::SEL } else { c::HOVER },
                        );
                    }
                    let color = if active { c::SEL_TEXT } else { c::TEXT_DIM };
                    (match mode {
                        ViewMode::Details => Icon::ViewDetails,
                        ViewMode::List => Icon::ViewList,
                        ViewMode::Large => Icon::ViewLarge,
                    })
                    .paint(ui.painter(), seg, color);
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

    /// Transient messages, stacked above the status bar on the right.
    pub(super) fn draw_toasts(&mut self, ctx: &Context) {
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

    pub(super) fn toast(&mut self, text: String) {
        self.toasts.push(Toast {
            text,
            danger: false,
            born: Instant::now(),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    pub(super) fn toast_err(&mut self, text: String) {
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

    pub(super) fn expire_toasts(&mut self) {
        let now = Instant::now();
        self.toasts
            .retain(|t| now.duration_since(t.born) < TOAST_TTL);
    }
}
