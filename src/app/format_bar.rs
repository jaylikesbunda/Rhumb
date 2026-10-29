//! The Markdown formatting bar, above the editor pane.
//!
//! A row of buttons that wrap and unwrap Markdown around the selection, so a
//! heading or a bold word is one click rather than a Remembered Punctuation
//! Exercise. The buttons do not edit anything themselves: each answers with
//! the [`MdAction`] it stands for, and the caller applies it through the
//! editor, so the toolbar and the shortcuts are the same path and every edit
//! lands in the same undo history.
//!
//! The pane the bar lives in can be dragged narrow - the split's floor is
//! 160 points, and a narrow window makes it less - so the bar gives its
//! buttons up from the right rather than let them hang out over the divider
//! and the preview. The order is therefore also the order of importance: the
//! inline styles survive the narrowest pane, the list markers are the first
//! to go. What is given up is not lost: a room for the dots is kept in
//! reserve, and the dots opens a menu of every action, shortcut hints
//! included, so the skinniest pane still reaches the whole list.
//!
//! The styling follows the rest of the app: no chrome until a button is
//! hovered, monochrome ink, and the icons come from the Phosphor font the
//! interface already speaks.

use egui::{CornerRadius, Ui, Vec2};

use crate::format::MdAction;
use crate::theme::{c, sp};
use crate::widgets::{Icon, icon_button, menu_item, menu_separator};

/// Height of the bar, taken off the top of the editor pane.
pub(super) const BAR_H: f32 = 30.0;

/// The square one button occupies, as [`icon_button`] draws it.
const BTN: f32 = 26.0;
/// The room one button needs with the spacing it keeps to its neighbours.
const BTN_ROOM: f32 = BTN + sp::SM;
/// The room a group's hairline needs with its spacing, plus the first button
/// behind it, since a divider is never left dangling over nothing.
const SEP_ROOM: f32 = 1.0 + sp::SM + sp::XS + BTN_ROOM;
/// The room kept in reserve for the dots, so that a pane narrow enough to
/// hide a button is never too narrow to reach what it hid.
const RESERVE: f32 = BTN_ROOM;

/// The bar's buttons, grouped the way hairlines group them: icon, name,
/// shortcut, action.
///
/// The groups are also given up in this order from the right, which is why
/// the inline styles come first: they are what every writer reaches for, and
/// the same table fills the dots menu when the pane hides them.
const GROUPS: &[&[(Icon, &str, &str, MdAction)]] = &[
    &[
        (
            Icon::Glyph(egui_phosphor::regular::TEXT_BOLDER),
            "Bold",
            "Ctrl+B",
            MdAction::Bold,
        ),
        (
            Icon::Glyph(egui_phosphor::regular::TEXT_ITALIC),
            "Italic",
            "Ctrl+I",
            MdAction::Italic,
        ),
        (
            Icon::Glyph(egui_phosphor::regular::TEXT_STRIKETHROUGH),
            "Strikethrough",
            "",
            MdAction::Strike,
        ),
        (
            Icon::Glyph(egui_phosphor::regular::CODE),
            "Inline code",
            "",
            MdAction::Code,
        ),
    ],
    &[(Icon::Link, "Link", "Ctrl+K", MdAction::Link)],
    &[
        (
            Icon::Glyph(egui_phosphor::regular::TEXT_H_ONE),
            "Heading 1",
            "",
            MdAction::Heading(1),
        ),
        (
            Icon::Glyph(egui_phosphor::regular::TEXT_H_TWO),
            "Heading 2",
            "",
            MdAction::Heading(2),
        ),
        (
            Icon::Glyph(egui_phosphor::regular::TEXT_H_THREE),
            "Heading 3",
            "",
            MdAction::Heading(3),
        ),
    ],
    &[
        (
            Icon::Glyph(egui_phosphor::regular::LIST_BULLETS),
            "Bullet list",
            "",
            MdAction::Bullet,
        ),
        (
            Icon::Glyph(egui_phosphor::regular::LIST_NUMBERS),
            "Numbered list",
            "",
            MdAction::Numbered,
        ),
        (
            Icon::Glyph(egui_phosphor::regular::LIST_CHECKS),
            "Task list",
            "",
            MdAction::Task,
        ),
        (
            Icon::Glyph(egui_phosphor::regular::QUOTES),
            "Block quote",
            "",
            MdAction::Quote,
        ),
    ],
];

/// Draws the bar across the top of `rect` when the document is Markdown.
///
/// Returns the strip of `rect` the bar did not use, which is where the editor
/// goes, and the action whose button was clicked this frame, if any. A
/// read-only document shows nothing to press, because there is nothing the
/// buttons could do.
pub(super) fn show(ui: &mut Ui, rect: egui::Rect, read_only: bool) -> (egui::Rect, Option<MdAction>) {
    let bar = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width(), BAR_H));
    let rest = egui::Rect::from_min_max(egui::Pos2::new(rect.min.x, bar.max.y), rect.max);
    if read_only {
        return (rest, None);
    }
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("md-format-bar"),
    );
    // A quiet strip the buttons sit on, so the bar reads as one tool whether
    // it is holding twelve buttons or two.
    ui.painter().rect_filled(bar, 0.0, c::PANEL);
    ui.add_space(sp::XS);

    let mut action = None;
    let mut hidden = false;
    'groups: for (g, group) in GROUPS.iter().enumerate() {
        if g > 0 {
            // The hairline between groups, only while the room for it, the
            // first button behind it and the reserve are all still there.
            if ui.available_width() < SEP_ROOM + RESERVE {
                hidden = true;
                break 'groups;
            }
            let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 16.0), egui::Sense::hover());
            ui.painter()
                .vline(r.center().x, r.y_range(), egui::Stroke::new(1.0, c::DIVIDER));
            ui.add_space(sp::XS);
        }
        for &(icon, label, hint, act) in group.iter() {
            // The pane is narrower than the bar is wide, and the button after
            // this one would hang past its edge - but only the last thing the
            // bar may draw is given up here, because the dots still have the
            // room kept for them.
            if ui.available_width() < BTN_ROOM + RESERVE {
                hidden = true;
                break 'groups;
            }
            let tip = if hint.is_empty() {
                label.to_owned()
            } else {
                format!("{label} ({hint})")
            };
            if icon_button(&mut ui, icon, &tip).clicked() {
                action = Some(act);
            }
        }
    }
    if hidden && ui.available_width() >= BTN_ROOM {
        overflow(&mut ui, &mut action);
    }
    (rest, action)
}

/// The dots, for the actions the pane was too narrow to show.
///
/// It opens the menu every formatting bar ends in: all of the actions, with
/// their shortcut hints at the right, so what a narrow pane hides stays one
/// click away and a reader can find the shortcuts while they are in there.
fn overflow(ui: &mut Ui, action: &mut Option<MdAction>) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(BTN, 24.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(sp::RADIUS), c::HOVER);
        }
        Icon::Glyph(egui_phosphor::regular::DOTS_THREE).paint(
            painter,
            rect,
            if resp.hovered() { c::TEXT } else { c::TEXT_DIM },
        );
    }
    let resp = resp.on_hover_text("Every style (the pane hid some)");
    egui::Popup::menu(&resp).show(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(sp::SM, 2.0);
        ui.set_min_width(210.0);
        for (g, group) in GROUPS.iter().enumerate() {
            if g > 0 {
                menu_separator(ui);
            }
            for &(icon, label, hint, act) in group.iter() {
                if menu_item(ui, icon, label, hint, false, true).clicked() {
                    *action = Some(act);
                }
            }
        }
    });
}

/// Whether the bar belongs over this document.
pub(super) fn wanted(kind: crate::editor::DocKind) -> bool {
    kind == crate::editor::DocKind::Markdown
}

/// The action behind a keyboard shortcut, for the keys the app owns while the
/// Markdown editor has the caret.
pub(super) fn action_of_key(key: egui::Key) -> Option<MdAction> {
    match key {
        egui::Key::B => Some(MdAction::Bold),
        egui::Key::I => Some(MdAction::Italic),
        egui::Key::K => Some(MdAction::Link),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect};

    /// Draws one frame of the bar at `width` and returns the bounding rect of
    /// everything it painted.
    ///
    /// The context needs the app's fonts, because the icons live in the
    /// Phosphor family the theme registers; and every frame's textures have to
    /// be cleared by hand, because there is no renderer here to apply them.
    fn drawn(width: f32, read_only: bool) -> Vec<Rect> {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::fonts());
        // One frame to build the font atlas, whose textures are cleared, the
        // way the theme's own glyph test does it.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 400.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let pane = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.0));
            let (rest, _) = show(ui, pane, read_only);
            assert_eq!(rest.top(), BAR_H, "the editor keeps what is under the bar");
            assert_eq!(rest.width(), width, "the strip spans the pane");
        });
        out.textures_delta.clear();
        out.shapes
            .iter()
            .map(|s| s.shape.visual_bounding_rect())
            .collect()
    }

    #[test]
    fn the_bar_draws_nothing_past_its_pane() {
        // From narrower than one button to wider than the whole bar.
        for width in [20.0, 80.0, 160.0, 240.0, 320.0, 480.0, 700.0] {
            let shapes = drawn(width, false);
            assert!(
                shapes.iter().all(|r| r.max.x <= width + 0.6),
                "at {width} points the bar painted past the pane: {shapes:?}"
            );
        }
    }

    #[test]
    fn a_narrow_pane_keeps_the_inline_styles_first() {
        // The point of the order: at the split's 160-point floor, bold is
        // still on screen, because it is the button a hand goes to first.
        let shapes = drawn(160.0, false);
        assert!(shapes.len() > 3, "the bar hid everything: {shapes:?}");
        assert!(
            shapes.iter().all(|r| r.max.x <= 160.6),
            "a button hung past the pane: {shapes:?}"
        );
    }

    #[test]
    fn a_pane_that_hides_buttons_offers_the_dots() {
        // The dots are the last thing a hiding bar draws, within a button's
        // pitch of the pane's right edge. The background strip spans the pane
        // either way, so it is the glyphs that are read here.
        let glyphs = |width: f32| {
            drawn(width, false)
                .into_iter()
                .filter(|r| r.width() < 100.0)
                .collect::<Vec<Rect>>()
        };
        for width in [80.0, 160.0, 240.0] {
            let rightmost = glyphs(width).iter().map(|r| r.max.x).fold(0.0f32, f32::max);
            assert!(
                rightmost >= width - BTN - 4.0 * sp::SM,
                "at {width} the dots are missing: rightmost {rightmost}"
            );
        }
        // Wide enough for everything: no dots, the bar simply ends.
        let rightmost = glyphs(700.0).iter().map(|r| r.max.x).fold(0.0f32, f32::max);
        assert!(
            rightmost < 700.0 - BTN - 4.0 * sp::SM,
            "the dots were drawn though nothing was hidden: {rightmost}"
        );
    }

    #[test]
    fn a_read_only_pane_draws_no_bar_at_all() {
        assert!(
            drawn(400.0, true).is_empty(),
            "a read-only document drew a bar"
        );
    }

    #[test]
    fn the_dots_menu_offers_every_action_exactly_once() {
        // Hidden must never mean unreachable, and the menu is the guarantee:
        // every action the bar can take is one of its rows.
        let mut seen: Vec<MdAction> = Vec::new();
        for group in GROUPS {
            for &(_, _, _, act) in group.iter() {
                assert!(!seen.contains(&act), "{act:?} is in the menu twice");
                seen.push(act);
            }
        }
        for act in [
            MdAction::Bold,
            MdAction::Italic,
            MdAction::Strike,
            MdAction::Code,
            MdAction::Link,
            MdAction::Heading(1),
            MdAction::Heading(2),
            MdAction::Heading(3),
            MdAction::Bullet,
            MdAction::Numbered,
            MdAction::Task,
            MdAction::Quote,
        ] {
            assert!(seen.contains(&act), "{act:?} is not in the menu");
        }
    }

    #[test]
    fn the_shortcut_hints_are_the_shortcuts_that_exist() {
        // A hint in the menu must never name a key the app does not answer,
        // and the three shortcuts the app owns must be hinted where they act.
        for group in GROUPS {
            for &(_, _, hint, act) in group.iter() {
                if hint.is_empty() {
                    continue;
                }
                let key = match act {
                    MdAction::Bold => egui::Key::B,
                    MdAction::Italic => egui::Key::I,
                    MdAction::Link => egui::Key::K,
                    other => panic!("{other:?} carries a shortcut hint but has no key"),
                };
                assert_eq!(action_of_key(key), Some(act), "{hint} does not match {act:?}");
            }
        }
        for (key, act) in [(egui::Key::B, MdAction::Bold), (egui::Key::I, MdAction::Italic), (egui::Key::K, MdAction::Link)] {
            let hinted = GROUPS
                .iter()
                .flat_map(|g| g.iter())
                .any(|&(_, _, hint, a)| !hint.is_empty() && a == act);
            assert!(hinted, "{act:?} answers {key:?} but no hint says so");
        }
    }
}
