//! The app's look (docs/design/m5.0-app.md, sections 8 and 10; decision 11): the values the
//! app sets (the palette, panel margins, bar heights, a list item's height, and corner
//! radii) as constants, applied to egui's style by one function of Silico's ([`apply`]);
//! and Inter as the one font ([`fonts`]). The layout is Fusion's, so only these values are
//! taken from Rerun, not its token files, their loader, or its mapping onto egui.
//!
//! The values are Rerun's design tokens, from `re_ui` 0.38.1 (`data/dark_theme.ron`,
//! `data/light_theme.ron`, `data/color_table.ron`, and `src/design_tokens.rs`). They are
//! Copyright (c) 2022 Rerun Technologies AB <opensource@rerun.io>, and used under its terms:
//! licensed under either of the Apache License, Version 2.0
//! (<http://www.apache.org/licenses/LICENSE-2.0>) or the MIT license
//! (<http://opensource.org/licenses/MIT>), at your option. Under the MIT license:
//!
//! > Permission is hereby granted, free of charge, to any person obtaining a copy of this
//! > software and associated documentation files (the "Software"), to deal in the Software
//! > without restriction, including without limitation the rights to use, copy, modify,
//! > merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject to the following
//! > conditions: The above copyright notice and this permission notice shall be included in
//! > all copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS",
//! > WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
//! > WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
//! > NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
//! > OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT
//! > OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
//!
//! Inter is the font's own, under the SIL Open Font License 1.1 (`silico-font`).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Margin, Stroke, Theme,
};

/// The colours the app sets, for one theme: each named as `re_ui`'s token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// `panel_bg_color`: the panels' background.
    pub panel_bg: Color32,
    /// `top_bar_color`: the application bar's.
    pub top_bar_bg: Color32,
    /// `bottom_bar_color`: the timeline's and the status bar's.
    pub bottom_bar_bg: Color32,
    /// `faint_bg_color`: a striped row, a hovered list item.
    pub faint_bg: Color32,
    /// `extreme_bg_color`: the canvas's place.
    pub extreme_bg: Color32,
    /// `text_edit_bg_color`.
    pub text_edit_bg: Color32,
    /// `widget_inactive_bg_fill`: a button at rest.
    pub widget_inactive_bg: Color32,
    /// `widget_hovered_color`: a button hovered.
    pub widget_hovered_bg: Color32,
    /// `widget_noninteractive_bg_stroke`: separators and frames.
    pub separator: Color32,
    /// `text_subdued`: a suppressed or unknown feature, a hint.
    pub text_subdued: Color32,
    /// `text_default`.
    pub text_default: Color32,
    /// `text_strong`: a hovered or active control's text.
    pub text_strong: Color32,
    /// `selection_bg_fill`.
    pub selection_bg: Color32,
    /// `selection_stroke_color`.
    pub selection_stroke: Color32,
    /// `error_fg_color`: a feature in error's mark (section 8, *The timeline*).
    pub error: Color32,
    /// `warn_fg_color`: a feature with a warning's mark.
    pub warning: Color32,
}

/// Rerun's dark theme (`dark_theme.ron`), each colour from its colour table.
pub const DARK: Palette = Palette {
    panel_bg: Color32::from_rgb(0x0d, 0x0d, 0x0d), // Gray.100
    top_bar_bg: Color32::from_rgb(0x0d, 0x0d, 0x0d), // Gray.100
    bottom_bar_bg: Color32::from_rgb(0x17, 0x17, 0x17), // Gray.150
    faint_bg: Color32::from_rgb(0x17, 0x17, 0x17), // Gray.150
    extreme_bg: Color32::from_rgb(0x00, 0x00, 0x00), // Gray.0
    text_edit_bg: Color32::from_rgb(0x21, 0x21, 0x21), // Gray.200
    widget_inactive_bg: Color32::from_rgb(0x2c, 0x2b, 0x2b), // Gray.300
    widget_hovered_bg: Color32::from_rgb(0x2c, 0x2b, 0x2b), // Gray.300
    separator: Color32::from_rgb(0x27, 0x26, 0x26), // Gray.250
    text_subdued: Color32::from_rgb(0x93, 0x90, 0x90), // Gray.550
    text_default: Color32::from_rgb(0xcf, 0xcf, 0xcf), // Gray.750
    text_strong: Color32::from_rgb(0xff, 0xff, 0xff), // Gray.1000
    selection_bg: Color32::from_rgb(0x18, 0x6a, 0xdd), // Blue.500
    selection_stroke: Color32::from_rgb(0xf0, 0xf2, 0xff), // Blue.900
    error: Color32::from_rgb(0xab, 0x01, 0x16),    // #AB0116
    warning: Color32::from_rgb(0xff, 0x7a, 0x0c),  // #FF7A0C
};

/// Rerun's light theme (`light_theme.ron`), each colour from its colour table; its
/// `widget_inactive_bg_fill`, which it doesn't set, as its hovered colour's neighbour.
pub const LIGHT: Palette = Palette {
    panel_bg: Color32::from_rgb(0xff, 0xff, 0xff), // Gray.1000
    top_bar_bg: Color32::from_rgb(0xe0, 0xdf, 0xdf), // Gray.800
    bottom_bar_bg: Color32::from_rgb(0xff, 0xff, 0xff), // Gray.1000
    faint_bg: Color32::from_rgb(0xee, 0xee, 0xed), // Gray.900
    extreme_bg: Color32::from_rgb(0xff, 0xff, 0xff), // Gray.1000
    text_edit_bg: Color32::from_rgb(0xee, 0xee, 0xed), // Gray.900
    widget_inactive_bg: Color32::from_rgb(0xe6, 0xe6, 0xe5), // Gray.850
    widget_hovered_bg: Color32::from_rgb(0xcf, 0xcf, 0xcf), // Gray.750
    separator: Color32::from_rgb(0xe0, 0xdf, 0xdf), // Gray.800
    text_subdued: Color32::from_rgb(0x82, 0x7f, 0x7f), // Gray.500
    text_default: Color32::from_rgb(0x27, 0x26, 0x26), // Gray.250
    text_strong: Color32::from_rgb(0x00, 0x00, 0x00), // Gray.0
    selection_bg: Color32::from_rgb(0x18, 0x6a, 0xdd), // Blue.500
    selection_stroke: Color32::from_rgb(0xf0, 0xf2, 0xff), // Blue.900
    error: Color32::from_rgb(0xab, 0x01, 0x16),    // #AB0116
    warning: Color32::from_rgb(0xff, 0x7a, 0x0c),  // #FF7A0C
};

/// Margin on every side of a view (`view_padding`).
pub const VIEW_PADDING: i8 = 12;

/// A panel's margin: the view's padding at its sides (`panel_margin`).
pub const PANEL_MARGIN: Margin = Margin::symmetric(VIEW_PADDING, 0);

/// The application bar's height (`top_bar_height`).
pub const TOP_BAR_HEIGHT: f32 = 28.0;

/// A pane's title row, and a tab bar's (`title_bar_height`).
pub const TITLE_BAR_HEIGHT: f32 = 24.0;

/// A row of the browser, or an entry of the timeline (`list_item_height`).
pub const LIST_ITEM_HEIGHT: f32 = 24.0;

/// Windows' corners (`window_corner_radius`).
pub const WINDOW_CORNER_RADIUS: u8 = 6;

/// Buttons' and frames' corners (`normal_corner_radius`).
pub const NORMAL_CORNER_RADIUS: u8 = 6;

/// Small controls' corners (`small_corner_radius`).
pub const SMALL_CORNER_RADIUS: u8 = 4;

/// The one font's name, as the fonts list it.
pub const FONT: &str = "Inter";

/// The fonts the app draws with: Inter alone, for proportional text and (until something
/// needs a monospace) monospace too; egui's default fonts aren't built in.
pub fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        FONT.to_owned(),
        Arc::new(FontData::from_static(silico_font::INTER_MEDIUM)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.insert(family, vec![FONT.to_owned()]);
    }
    fonts
}

/// The app's look applied to `ctx`: Inter, and each theme's palette and sizes on egui's
/// own style for that theme. Which theme shows follows the system's, egui's default.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_style_of(Theme::Dark, style(egui::Style::default(), &DARK, true));
    ctx.set_style_of(Theme::Light, style(egui::Style::default(), &LIGHT, false));
}

/// `base` with `palette` and the sizes set: the app's style for one theme.
pub fn style(mut base: egui::Style, palette: &Palette, dark: bool) -> egui::Style {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = palette.panel_bg;
    visuals.window_fill = palette.panel_bg;
    visuals.faint_bg_color = palette.faint_bg;
    visuals.extreme_bg_color = palette.extreme_bg;
    visuals.text_edit_bg_color = Some(palette.text_edit_bg);
    visuals.code_bg_color = palette.text_edit_bg;
    visuals.error_fg_color = palette.error;
    visuals.warn_fg_color = palette.warning;
    visuals.selection.bg_fill = palette.selection_bg;
    visuals.selection.stroke = Stroke::new(1.0, palette.selection_stroke);
    visuals.window_corner_radius = CornerRadius::same(WINDOW_CORNER_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(WINDOW_CORNER_RADIUS);
    visuals.window_stroke = Stroke::new(1.0, palette.separator);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = palette.panel_bg;
    widgets.noninteractive.weak_bg_fill = palette.panel_bg;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.separator);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text_default);
    for (state, fill, text) in [
        (
            &mut widgets.inactive,
            palette.widget_inactive_bg,
            palette.text_default,
        ),
        (
            &mut widgets.hovered,
            palette.widget_hovered_bg,
            palette.text_strong,
        ),
        (
            &mut widgets.active,
            palette.widget_hovered_bg,
            palette.text_strong,
        ),
        (
            &mut widgets.open,
            palette.widget_inactive_bg,
            palette.text_default,
        ),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = Stroke::new(1.0, text);
        state.corner_radius = CornerRadius::same(SMALL_CORNER_RADIUS);
    }
    base.visuals = visuals;
    base.spacing.interact_size.y = LIST_ITEM_HEIGHT - 4.0;
    base.spacing.window_margin = Margin::same(VIEW_PADDING);
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_is_the_one_font_for_every_family() {
        let fonts = fonts();
        assert_eq!(
            fonts.font_data.keys().collect::<Vec<_>>(),
            [&FONT.to_owned()]
        );
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert_eq!(fonts.families[&family], [FONT.to_owned()]);
        }
    }

    #[test]
    fn each_theme_sets_its_palette_on_egui() {
        for (palette, dark) in [(&DARK, true), (&LIGHT, false)] {
            let style = style(egui::Style::default(), palette, dark);
            assert_eq!(style.visuals.dark_mode, dark);
            assert_eq!(style.visuals.panel_fill, palette.panel_bg);
            assert_eq!(
                style.visuals.error_fg_color,
                Color32::from_rgb(0xab, 0x01, 0x16)
            );
            assert_eq!(
                style.visuals.warn_fg_color,
                Color32::from_rgb(0xff, 0x7a, 0x0c)
            );
            assert_eq!(
                style.visuals.widgets.inactive.fg_stroke.color,
                palette.text_default
            );
        }
    }
}
