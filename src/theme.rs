//! Design tokens: palette, spacing, typography and font loading.
//!
//! Every colour, gap and column width used anywhere in the app comes from this
//! module, so the interface stays visually consistent and stays monochrome.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Frame, Margin,
    Stroke, Style, TextStyle,
    style::{WidgetVisuals, Widgets},
};
use std::sync::Arc;

/// Strictly neutral (achromatic) palette. Every colour below has identical red,
/// green and blue channels, which a unit test enforces. The single exception is
/// the desaturated warning tint used for destructive actions.
pub mod c {
    use egui::Color32;

    /// A pure grey: every channel identical, which is what makes the palette
    /// monochrome. Keeping one helper means the rule is visible at a glance.
    const fn grey(v: u8) -> Color32 {
        Color32::from_rgb(v, v, v)
    }

    /// Window background / list backdrop.
    pub const BG: Color32 = grey(0x0E);
    /// Chrome: toolbar, sidebar, status bar.
    pub const PANEL: Color32 = grey(0x15);
    /// Raised surfaces: menus, dialogs, hovered rows.
    pub const RAISED: Color32 = grey(0x1E);
    /// Row hover.
    pub const HOVER: Color32 = grey(0x24);
    /// Hard separators (panel edges, dialog borders).
    pub const BORDER: Color32 = grey(0x2E);
    /// Soft separators inside panels.
    pub const DIVIDER: Color32 = grey(0x25);
    /// Code and inline-code background.
    pub const CODE_BG: Color32 = grey(0x12);
    /// Primary text.
    pub const TEXT: Color32 = grey(0xE8);
    /// Secondary text: sizes, dates, subtitles.
    pub const TEXT_DIM: Color32 = grey(0x9C);
    /// Tertiary text: hints, placeholders.
    pub const TEXT_FAINT: Color32 = grey(0x6E);
    /// Barely-there text: overflow markers, empty states.
    pub const TEXT_GHOST: Color32 = grey(0x4A);
    /// Highest-contrast neutral, for selection bars and focus rings.
    pub const ACCENT: Color32 = grey(0xF7);
    /// Selected row background.
    pub const SEL: Color32 = grey(0x30);
    /// Text on top of [`SEL`].
    pub const SEL_TEXT: Color32 = Color32::WHITE;
    /// Cursor / caret colour.
    pub const CARET: Color32 = grey(0x93);
    /// Destructive-action text. The only non-grey in the app.
    pub const DANGER: Color32 = Color32::from_rgb(0xC2, 0x92, 0x92);
}

/// Spacing scale. Use these instead of magic numbers so rhythm is identical
/// across every panel.
pub mod sp {
    /// 4px — icon-to-text gap.
    pub const XS: f32 = 4.0;
    /// 8px — inner padding, list row horizontal padding.
    pub const SM: f32 = 8.0;
    /// 12px — block spacing.
    pub const MD: f32 = 12.0;
    /// 16px — section spacing, dialog padding.
    pub const LG: f32 = 16.0;
    /// 24px — major separation.
    pub const XL: f32 = 24.0;

    /// File list row height.
    pub const ROW: f32 = 26.0;
    /// Title bar height.
    pub const TITLE: f32 = 30.0;
    /// Toolbar height.
    pub const TOOLBAR: f32 = 38.0;
    /// Status bar height.
    pub const STATUS: f32 = 24.0;
    /// Tab strip height.
    pub const TAB_H: f32 = 26.0;
    /// Sidebar section header height.
    pub const SECTION: f32 = 24.0;
    /// Icon square edge length.
    pub const ICON: f32 = 14.0;
    /// Corner radius for small controls.
    pub const RADIUS: u8 = 5;
    /// Corner radius for dialogs and menus.
    pub const RADIUS_LG: u8 = 8;
    /// Sidebar row height.
    pub const NAV_ROW: f32 = 26.0;
    /// List nesting indent.
    pub const INDENT: f32 = 16.0;
    /// Window control button width (Windows-style hit area).
    pub const WIN_BTN_W: f32 = 46.0;
    /// Max comfortable measure for rendered Markdown.
    pub const MD_MEASURE: f32 = 760.0;
    /// Tile width in the large-icon view.
    pub const TILE_W: f32 = 104.0;
    /// Tile height in the large-icon view: artwork plus one line of name.
    pub const TILE: f32 = 86.0;

    /// Integer forms, for egui APIs that take `i8` margins.
    pub const SM_I: i8 = 8;
    /// Integer form of [`Self::LG`].
    pub const LG_I: i8 = 16;
}

/// Column widths in the file list. Fixed so numbers and dates always line up,
/// and both are draggable in the header, within these limits.
pub mod col {
    /// Right-aligned modified-date column.
    pub const DATE: f32 = 132.0;
    /// Right-aligned size column.
    pub const SIZE: f32 = 92.0;
    /// Narrowest a draggable column may become.
    pub const MIN: f32 = 48.0;
    /// Widest a draggable column may become.
    pub const MAX: f32 = 320.0;
    /// Half-width of the grab area around a divider.
    pub const GRAB: f32 = 4.0;
}

/// Type scale (logical pixels).
pub mod fs {
    /// Body text.
    pub const BODY: f32 = 13.0;
    /// Small text: status bar, hints.
    pub const SMALL: f32 = 11.5;
    /// Monospace text.
    pub const MONO: f32 = 12.5;
    /// H1.
    pub const H1: f32 = 21.0;
    /// H2.
    pub const H2: f32 = 17.0;
    /// H3.
    pub const H3: f32 = 14.5;
    /// H4-H6.
    pub const H4: f32 = 13.0;
}

pub(crate) const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
pub(crate) const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
const JBMONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const JBMONO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");

/// Font families registered in the app.
pub const FAMILY_UI: &str = "rhumb-ui";
pub const FAMILY_UI_BOLD: &str = "rhumb-ui-bold";
pub const FAMILY_MONO: &str = "rhumb-mono";
pub const FAMILY_MONO_BOLD: &str = "rhumb-mono-bold";

/// Loads Inter (UI) and JetBrains Mono (code) ahead of egui's bundled fallbacks,
/// so icons and symbols we do not ship still render.
pub fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    // Hinting off keeps small UI text crisp and consistent across platforms.
    let tweak = || FontTweak {
        hinting: Some(false),
        ..FontTweak::default()
    };

    defs.font_data.insert(
        FAMILY_UI.to_owned(),
        Arc::new(FontData::from_static(INTER_REGULAR).tweak(tweak())),
    );
    defs.font_data.insert(
        FAMILY_UI_BOLD.to_owned(),
        Arc::new(FontData::from_static(INTER_SEMIBOLD).tweak(tweak())),
    );
    defs.font_data.insert(
        FAMILY_MONO.to_owned(),
        Arc::new(FontData::from_static(JBMONO_REGULAR).tweak(tweak())),
    );
    defs.font_data.insert(
        FAMILY_MONO_BOLD.to_owned(),
        Arc::new(FontData::from_static(JBMONO_MEDIUM).tweak(tweak())),
    );

    // egui's own families stay last in the chain, so glyphs we do not ship
    // (arrows, bullets, symbols) still render.
    let fallbacks =
        |key: &FontFamily| -> Vec<String> { defs.families.get(key).cloned().unwrap_or_default() };
    let prop_fallbacks = fallbacks(&FontFamily::Proportional);
    let mono_fallbacks = fallbacks(&FontFamily::Monospace);

    let mut proportional = vec![FAMILY_UI.to_owned(), FAMILY_UI_BOLD.to_owned()];
    proportional.extend(prop_fallbacks.iter().cloned());

    let mut monospace = vec![
        FAMILY_MONO.to_owned(),
        FAMILY_MONO_BOLD.to_owned(),
        FAMILY_UI.to_owned(),
    ];
    monospace.extend(mono_fallbacks.iter().cloned());

    // Named families used by the font helpers below. Each one must be bound,
    // otherwise epaint panics when a `FontId` points at it.
    let mut ui_bold = vec![FAMILY_UI_BOLD.to_owned(), FAMILY_UI.to_owned()];
    ui_bold.extend(prop_fallbacks.iter().cloned());
    let mut mono_bold = vec![FAMILY_MONO_BOLD.to_owned(), FAMILY_MONO.to_owned()];
    mono_bold.extend(mono_fallbacks.iter().cloned());
    let mut ui_only = vec![FAMILY_UI.to_owned(), FAMILY_UI_BOLD.to_owned()];
    ui_only.extend(prop_fallbacks.iter().cloned());
    let mut mono_only = vec![FAMILY_MONO.to_owned()];
    mono_only.extend(mono_fallbacks.iter().cloned());

    // The icons: their own family, so a glyph's codepoint is never taken for text.
    defs.font_data.insert(
        crate::widgets::ICON_FAMILY.to_owned(),
        Arc::new(FontData::from_static(
            egui_phosphor::Variant::Regular.font_bytes(),
        )),
    );
    defs.families.insert(
        FontFamily::Name(crate::widgets::ICON_FAMILY.into()),
        vec![crate::widgets::ICON_FAMILY.to_owned()],
    );

    defs.families.insert(FontFamily::Proportional, proportional);
    defs.families.insert(FontFamily::Monospace, monospace);
    defs.families
        .insert(FontFamily::Name(FAMILY_UI.into()), ui_only);
    defs.families
        .insert(FontFamily::Name(FAMILY_UI_BOLD.into()), ui_bold);
    defs.families
        .insert(FontFamily::Name(FAMILY_MONO.into()), mono_only);
    defs.families
        .insert(FontFamily::Name(FAMILY_MONO_BOLD.into()), mono_bold);

    defs
}

/// Font id helpers, so sizes and families are never spelled out inline.
pub fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Semi-bold UI font (labels, headings, toolbar).
pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_UI_BOLD.into()))
}

/// Monospace font.
pub fn mono_font(size: f32) -> FontId {
    FontId::monospace(size)
}

fn widget(fill: Color32, stroke: Option<Color32>, fg: Color32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: stroke.map_or(Stroke::NONE, |s| Stroke::new(1.0, s)),
        corner_radius: CornerRadius::same(sp::RADIUS),
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    }
}

/// The one and only style definition.
pub fn style() -> Style {
    let mut s = Style::default();

    let mut widgets = Widgets {
        // Non-interactive widgets must never paint a filled box.
        noninteractive: widget(c::BG, None, c::TEXT),
        inactive: widget(Color32::TRANSPARENT, None, c::TEXT_DIM),
        hovered: widget(c::HOVER, Some(c::BORDER), c::TEXT),
        active: widget(c::SEL, Some(c::BORDER), c::ACCENT),
        open: widget(c::SEL, Some(c::BORDER), c::ACCENT),
    };
    widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    s.visuals.widgets = widgets;

    s.visuals.dark_mode = true;
    s.visuals.override_text_color = Some(c::TEXT);
    s.visuals.panel_fill = c::BG;
    s.visuals.window_fill = c::RAISED;
    s.visuals.faint_bg_color = c::RAISED;
    s.visuals.extreme_bg_color = c::CODE_BG;
    s.visuals.code_bg_color = c::CODE_BG;
    s.visuals.text_edit_bg_color = Some(c::BG);
    s.visuals.window_stroke = Stroke::new(1.0, c::BORDER);
    s.visuals.window_corner_radius = CornerRadius::same(sp::RADIUS_LG);
    s.visuals.menu_corner_radius = CornerRadius::same(sp::RADIUS);
    s.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    s.visuals.window_shadow = egui::epaint::Shadow::NONE;
    s.visuals.hyperlink_color = c::ACCENT;
    s.visuals.selection.bg_fill = c::SEL;
    s.visuals.selection.stroke = Stroke::new(1.0, c::ACCENT);
    s.visuals.weak_text_color = Some(c::TEXT_FAINT);
    s.visuals.weak_text_alpha = 1.0;
    s.visuals.warn_fg_color = c::DANGER;
    s.visuals.error_fg_color = c::DANGER;
    s.visuals.text_cursor.stroke = Stroke::new(1.0, c::CARET);

    s.spacing.item_spacing = egui::vec2(sp::SM, sp::XS);
    s.spacing.button_padding = egui::vec2(sp::SM, 5.0);
    s.spacing.menu_margin = Margin::same(4);
    s.spacing.window_margin = Margin::same(sp::SM_I);
    s.spacing.indent = sp::INDENT;
    s.spacing.interact_size.y = sp::ROW;
    // Scroll bars are thin and quiet until the pointer comes near, then grow to a wide,
    // easy target. egui widens a floating bar when the pointer is over the space it
    // will occupy, so the width it grows to is also how close the pointer has to get:
    // 16 points here, which is a comfortable throw for anyone reaching for the edge.
    // A bar that is only visible on hover is one nobody can find, so a resting bar is
    // still drawn, faintly.
    s.spacing.scroll.floating = true;
    s.spacing.scroll.bar_width = 16.0;
    s.spacing.scroll.floating_width = 4.0;
    s.spacing.scroll.floating_allocated_width = 6.0;
    s.spacing.scroll.handle_min_length = 32.0;
    s.spacing.scroll.foreground_color = true;
    s.spacing.scroll.dormant_background_opacity = 0.0;
    s.spacing.scroll.active_background_opacity = 0.35;
    s.spacing.scroll.interact_background_opacity = 0.7;
    s.spacing.scroll.dormant_handle_opacity = 0.45;
    s.spacing.scroll.active_handle_opacity = 0.75;
    s.spacing.scroll.interact_handle_opacity = 1.0;
    s.spacing.menu_width = 190.0;
    s.spacing.combo_width = 160.0;
    s.spacing.tooltip_width = 320.0;

    s.text_styles.insert(TextStyle::Body, ui_font(fs::BODY));
    s.text_styles.insert(TextStyle::Small, ui_font(fs::SMALL));
    s.text_styles
        .insert(TextStyle::Monospace, mono_font(fs::MONO));
    s.text_styles.insert(TextStyle::Button, ui_font(fs::BODY));
    s.text_styles.insert(TextStyle::Heading, bold_font(fs::H3));

    s
}

/// Frame used by modal dialogs.
pub fn dialog_frame() -> Frame {
    Frame::popup(&Style::default())
        .fill(c::RAISED)
        .stroke(Stroke::new(1.0, c::BORDER))
        .corner_radius(CornerRadius::same(sp::RADIUS_LG))
        .inner_margin(Margin::same(sp::LG_I))
        .shadow(egui::epaint::Shadow::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_id_points_at_a_bound_family() {
        // A `FontId` naming an unregistered family makes epaint panic at paint
        // time, which is much too late to catch in review.
        let defs = fonts();
        let ids = [
            ui_font(fs::BODY),
            bold_font(fs::BODY),
            mono_font(fs::MONO),
            FontId::new(fs::BODY, FontFamily::Name(FAMILY_MONO_BOLD.into())),
        ];
        for id in ids {
            assert!(
                defs.families.contains_key(&id.family),
                "family {:?} is not bound to any font",
                id.family
            );
        }
    }

    #[test]
    fn all_custom_families_have_font_data() {
        let defs = fonts();
        for name in [FAMILY_UI, FAMILY_UI_BOLD, FAMILY_MONO, FAMILY_MONO_BOLD] {
            assert!(defs.font_data.contains_key(name), "{name} has no font data");
        }
    }

    #[test]
    fn every_ui_symbol_is_in_the_font() {
        // A symbol no font in the chain covers renders as a tofu box, and it
        // looks fine in code review. Only glyphs the UI actually uses belong
        // below: the New button once used U+25BE, which neither Inter nor the
        // fallback covers, and now uses U+25BC. This walks the real family
        // chain, so a glyph that renders is proven to render.
        const SYMBOLS: &[(&str, char)] = &[
            ("disclosure triangle", '\u{25BC}'),
            ("sort ascending", '\u{2191}'),
            ("sort descending", '\u{2193}'),
            ("symlink arrow", '\u{2192}'),
            ("filter on", '\u{25CF}'),
            ("filter off", '\u{25CB}'),
            ("ellipsis", '\u{2026}'),
            ("em dash", '\u{2014}'),
        ];
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts());
        // egui builds its font set lazily on the first run, and `fonts_mut`
        // panics before that, so give it one empty frame first. The glyph
        // lookups then allocate atlas textures, which have to be cleared
        // before the context drops or epaint complains about them.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let id = ui_font(fs::BODY);
        let missing: Vec<&str> = SYMBOLS
            .iter()
            .filter(|(_, ch)| !ctx.fonts_mut(|f| f.has_glyph(&id, *ch)))
            .map(|(name, _)| *name)
            .collect();
        assert!(
            missing.is_empty(),
            "no font in the chain covers: {}",
            missing.join(", ")
        );
    }

    #[test]
    fn palette_is_exactly_monochrome() {
        // Every palette entry has identical channels. Anything with a cast
        // belongs in the one `DANGER` exception, not here.
        for (name, col) in [
            ("BG", c::BG),
            ("PANEL", c::PANEL),
            ("RAISED", c::RAISED),
            ("HOVER", c::HOVER),
            ("BORDER", c::BORDER),
            ("DIVIDER", c::DIVIDER),
            ("CODE_BG", c::CODE_BG),
            ("TEXT", c::TEXT),
            ("TEXT_DIM", c::TEXT_DIM),
            ("TEXT_FAINT", c::TEXT_FAINT),
            ("TEXT_GHOST", c::TEXT_GHOST),
            ("ACCENT", c::ACCENT),
            ("SEL", c::SEL),
            ("SEL_TEXT", c::SEL_TEXT),
            ("CARET", c::CARET),
        ] {
            assert_eq!(
                (col.r(), col.g(), col.b()),
                (col.r(), col.r(), col.r()),
                "{name} is not monochrome: {col:?}"
            );
        }
    }

    #[test]
    fn the_only_tinted_colour_is_the_destructive_one() {
        assert!(c::DANGER.r() > c::DANGER.b(), "DANGER should read as warm");
    }

    #[test]
    fn style_is_dark() {
        let s = style();
        assert!(s.visuals.dark_mode);
        assert_eq!(s.visuals.panel_fill, c::BG);
        assert!(s.spacing.item_spacing.x >= 8.0);
    }
}
