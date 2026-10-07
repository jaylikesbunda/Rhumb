//! A row of tabs: the same drawing for the folder tabs along the top of the window and for
//! the documents above the editor.
//!
//! It knows nothing of what the tabs are. It is given a list of items to show and says what
//! was done to them, and the caller does it: bringing one forward, closing one, moving one,
//! opening a new one.

use super::*;
use egui::{WidgetInfo, WidgetType};

/// One tab to draw.
pub(super) struct Item {
    /// A number that stays with the tab however the row is rearranged.
    pub id: u64,
    pub label: String,
    pub icon: Icon,
    /// Whether there is something unsaved behind it.
    pub dirty: bool,
}

/// What a row remembers between frames.
#[derive(Default)]
pub(super) struct State {
    /// How far the row is slid sideways, when there are more tabs than fit.
    pub scroll: f32,
    /// Where each tab was drawn on the last frame, in order.
    pub rects: Vec<Rect>,
    /// The tab that was in front when the row last looked, so that a change brings it
    /// into view.
    seen_active: Option<usize>,
}

/// What was done to the row on this frame.
#[derive(Default, Debug, PartialEq)]
pub(super) struct Action {
    /// A tab was pressed: it comes forward.
    pub focus: Option<usize>,
    /// A tab's cross was pressed, or its middle button.
    pub close: Option<usize>,
    /// A tab was dragged to another place: from, to.
    pub reorder: Option<(usize, usize)>,
    /// The button that opens a new tab was pressed.
    pub add: bool,
    /// A tab was picked from the list of all of them.
    pub pick: Option<usize>,
}

/// How the row is set out.
pub(super) struct Style<'a> {
    /// A separate name for each row, so two of them never share a widget id.
    pub salt: &'a str,
    /// What the new tab button says, or `None` for a row without one.
    pub new_tip: Option<&'a str>,
    /// Tabs never get narrower than this; the row slides instead.
    pub min_w: f32,
    pub max_w: f32,
}

/// Draws the row in `rect` and returns what was done to it.
pub(super) fn show(
    ui: &mut Ui,
    rect: Rect,
    items: &[Item],
    active: usize,
    state: &mut State,
    style: &Style<'_>,
) -> Action {
    let mut action = Action::default();
    let count = items.len();
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.max.y - 0.5,
        Stroke::new(1.0, c::BORDER),
    );
    state.rects.clear();
    if count == 0 {
        return action;
    }
    const BUTTON_W: f32 = 28.0;
    let close_w = 16.0f32;
    // The room for the tabs: the row less the buttons at its end, the one that opens a tab
    // and, when there are too many to show, the one that lists them all.
    let has_new = style.new_tip.is_some();
    let plain_room = rect.width() - 4.0 - if has_new { BUTTON_W } else { 0.0 };
    let overflow = count as f32 * style.min_w > plain_room;
    let room = plain_room - if overflow { BUTTON_W } else { 0.0 };
    let w = (room / count as f32).clamp(style.min_w, style.max_w);
    let total = w * count as f32;
    let max_scroll = (total - room).max(0.0);
    let area = Rect::from_min_size(
        Pos2::new(rect.left() + 2.0, rect.top()),
        Vec2::new(room, rect.height()),
    );
    // A tab that has just come to the front is brought into view.
    if state.seen_active != Some(active) {
        state.seen_active = Some(active);
        let left = active as f32 * w;
        if left < state.scroll {
            state.scroll = left;
        } else if left + w > state.scroll + room {
            state.scroll = left + w - room;
        }
    }
    // The wheel over the row slides it sideways.
    if overflow
        && ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| area.contains(p))
    {
        let d = ui.input(|i| i.smooth_scroll_delta);
        state.scroll -= if d.x.abs() > d.y.abs() { d.x } else { d.y };
    }
    state.scroll = state.scroll.clamp(0.0, max_scroll);
    let scroll = state.scroll;
    let strip = ui.painter().with_clip_rect(area);
    let mut rects: Vec<Rect> = Vec::with_capacity(count);

    for (i, item) in items.iter().enumerate() {
        let x = area.left() + i as f32 * w - scroll;
        let tab_rect = Rect::from_min_size(
            Pos2::new(x, rect.top() + 2.0),
            Vec2::new(w - 2.0, rect.height() - 3.0),
        );
        rects.push(tab_rect);
        if tab_rect.right() < area.left() || tab_rect.left() > area.right() {
            continue;
        }
        // Only the part of the tab that is in the row answers the pointer.
        let hit = tab_rect.intersect(area);
        let resp = ui.interact(
            hit,
            Id::new((style.salt, "tab", item.id)),
            Sense::click_and_drag(),
        );
        // Pressing a tab brings it forward, as in a browser. Waiting for a click to finish
        // made a press that moved a few pixels no click at all, and the tab did not respond.
        if resp.drag_started() || resp.clicked() {
            action.focus = Some(i);
        }
        if resp.drag_stopped()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Let go over another place in the row: the tab goes there.
            let to = (((p.x - area.left() + scroll) / w).floor().max(0.0) as usize).min(count - 1);
            if to != i {
                action.reorder = Some((i, to));
            }
        }
        if resp.middle_clicked() {
            action.close = Some(i);
        }

        let is_active = i == active;
        // A tab is a selectable button named by its label, so a screen reader
        // can tell which one is in front.
        resp.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, is_active, item.label.as_str())
        });
        if is_active {
            strip.rect_filled(tab_rect, CornerRadius::same(sp::RADIUS), c::SEL);
        } else if resp.hovered() {
            strip.rect_filled(tab_rect, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        // The active tab is joined to the body below it by a hairline.
        if is_active {
            strip.hline(
                tab_rect.left()..=tab_rect.right(),
                tab_rect.max.y - 0.5,
                Stroke::new(1.0, c::BG),
            );
        }

        let text_color = if is_active { c::SEL_TEXT } else { c::TEXT_DIM };
        let icon = Rect::from_center_size(
            Pos2::new(tab_rect.left() + 12.0, tab_rect.center().y),
            Vec2::splat(sp::ICON),
        );
        item.icon.paint(&strip, icon, text_color);

        let text_x = icon.right() + 6.0;
        let name_w = (tab_rect.right() - text_x - close_w - 6.0).max(20.0);
        let name_g = widgets::layout_elided(
            ui,
            item.label.clone(),
            theme::ui_font(tfs::SMALL),
            text_color,
            name_w,
        );
        widgets::galley_at(
            &strip,
            Pos2::new(text_x, tab_rect.center().y - name_g.size().y * 0.5),
            &name_g,
            text_color,
        );

        // Unsaved changes: a dot, the same signal as the header uses.
        if item.dirty {
            strip.circle_filled(
                Pos2::new(text_x + name_g.size().x + 5.0, tab_rect.center().y),
                2.5,
                c::ACCENT,
            );
        }

        // The close cross, which lights up under the pointer.
        let close_rect = Rect::from_center_size(
            Pos2::new(tab_rect.right() - 11.0, tab_rect.center().y),
            Vec2::splat(close_w),
        );
        if close_rect.intersects(area) {
            let close_resp = ui.interact(
                close_rect.intersect(area),
                Id::new((style.salt, "close", item.id)),
                Sense::click(),
            );
            close_resp.widget_info(|| {
                WidgetInfo::labeled(
                    WidgetType::Button,
                    true,
                    if item.dirty {
                        "Close without saving"
                    } else {
                        "Close"
                    },
                )
            });
            if close_resp.clicked() {
                action.close = Some(i);
            }
            if close_resp.hovered() || resp.hovered() || is_active {
                Icon::Close.paint(
                    &strip,
                    close_rect,
                    if close_resp.hovered() {
                        c::TEXT
                    } else {
                        c::TEXT_DIM
                    },
                );
            }
            let _ = close_resp.on_hover_text(if item.dirty {
                "Close without saving"
            } else {
                "Close"
            });
        }
    }
    state.rects = rects;

    // At the end of the tabs: the button that opens a new one, and when they run past the
    // edge, the one that lists them all.
    if let Some(tip) = style.new_tip {
        let after = (area.left() + total - scroll).min(area.right()) + 2.0;
        let plus = Rect::from_min_size(
            Pos2::new(after, rect.top() + 3.0),
            Vec2::new(24.0, rect.height() - 6.0),
        );
        let resp = ui.interact(plus, Id::new((style.salt, "new")), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, tip));
        if resp.hovered() {
            ui.painter()
                .rect_filled(plus, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        Icon::Glyph(egui_phosphor::regular::PLUS).paint(
            ui.painter(),
            plus,
            if resp.hovered() { c::TEXT } else { c::TEXT_DIM },
        );
        if resp.on_hover_text(tip).clicked() {
            action.add = true;
        }
    }
    if overflow {
        let list = Rect::from_min_size(
            Pos2::new(rect.right() - BUTTON_W - 2.0, rect.top() + 3.0),
            Vec2::new(24.0, rect.height() - 6.0),
        );
        let resp = ui.interact(list, Id::new((style.salt, "list")), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "All tabs"));
        if resp.hovered() {
            ui.painter()
                .rect_filled(list, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        Icon::Glyph(egui_phosphor::regular::CARET_DOWN).paint(
            ui.painter(),
            list,
            if resp.hovered() { c::TEXT } else { c::TEXT_DIM },
        );
        let resp = resp.on_hover_text("All tabs");
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.0);
            for (i, item) in items.iter().enumerate() {
                if ui.selectable_label(i == active, &item.label).clicked() {
                    action.pick = Some(i);
                }
            }
        });
    }
    action
}
