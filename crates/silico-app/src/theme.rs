//! The app's look (docs/design/m5.0-app.md, sections 8 and 10; decision 11), as the approved
//! UI prototype has it (docs/design/m5-ui/DESIGN.md, *Tokens*, *Type*, and *Spacing*, and
//! its `styles.css`): every token as a named constant, grouped by where it's used, applied
//! to egui's style by one function of Silico's ([`apply`]); Inter for text and JetBrains
//! Mono for IDs and element names ([`fonts`]). The layout is Fusion's, so only these values
//! are taken from Rerun, not its token files, their loader, or its mapping onto egui.
//!
//! The look is dark only: the prototype gives no light values. Its colours are Rerun's
//! dark theme, resolved through its colour table, but for the few the prototype proposes
//! and the maintainer approved (2026-11-01): a brighter red for error text, a brighter blue
//! for focus, and the invalid, dangling, and changed-outside colours `re_ui` lacks. Each
//! constant says which it is. The viewport's values are plain `[u8; 4]`s ([`Rgba8`]), which
//! the app passes to `silico-render`'s frames; [`color32`] gives egui's form of one. No
//! lighting value is here: those are `silico-render`'s alone.
//!
//! The values are Rerun's design tokens, from `re_ui` 0.38.1 (`data/dark_theme.ron`,
//! `data/color_table.ron`, and `src/design_tokens.rs`). They are
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
//! Inter and JetBrains Mono are their own, each under the SIL Open Font License 1.1
//! (`silico-font`).

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle, Theme, ThemePreference, Vec2,
};

// ---- Colour forms ----

/// A colour as the renderer takes it: sRGB red, green, and blue, and alpha (not
/// premultiplied).
pub type Rgba8 = [u8; 4];

/// `rgba` as egui's colour.
pub const fn color32(rgba: Rgba8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(rgba[0], rgba[1], rgba[2], rgba[3])
}

/// An opaque colour from its hex value, `0xrrggbb`.
const fn hex(rgb: u32) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

/// `0xrrggbb` with `alpha` of 255, not premultiplied.
const fn hex_alpha(rgb: u32, alpha: u8) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgba_unmultiplied_const(r, g, b, alpha)
}

/// An opaque [`Rgba8`] from its hex value, `0xrrggbb`.
const fn rgba8(rgb: u32) -> Rgba8 {
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b, 0xff]
}

// ---- Rerun's greys (`color_table.ron`'s Gray scale), which the tokens below name ----

/// Gray.0.
pub const GRAY_0: Color32 = hex(0x000000);
/// Gray.100.
pub const GRAY_100: Color32 = hex(0x0d0d0d);
/// Gray.150.
pub const GRAY_150: Color32 = hex(0x171717);
/// Gray.200.
pub const GRAY_200: Color32 = hex(0x212121);
/// Gray.250.
pub const GRAY_250: Color32 = hex(0x272626);
/// Gray.300.
pub const GRAY_300: Color32 = hex(0x2c2b2b);
/// Gray.350.
pub const GRAY_350: Color32 = hex(0x525151);
/// Gray.400.
pub const GRAY_400: Color32 = hex(0x646363);
/// Gray.500.
pub const GRAY_500: Color32 = hex(0x827f7f);
/// Gray.550.
pub const GRAY_550: Color32 = hex(0x939090);
/// Gray.700.
pub const GRAY_700: Color32 = hex(0xbfbfbf);
/// Gray.750.
pub const GRAY_750: Color32 = hex(0xcfcfcf);
/// Gray.800.
pub const GRAY_800: Color32 = hex(0xe0dfdf);
/// Gray.850.
pub const GRAY_850: Color32 = hex(0xe6e6e5);
/// Gray.900.
pub const GRAY_900: Color32 = hex(0xeeeeed);
/// Gray.1000.
pub const GRAY_1000: Color32 = hex(0xffffff);

// ---- Surfaces ----

/// The app bar's and the toolbar's background (`top_bar_color`, Gray.100).
pub const TOP_BAR_BG: Color32 = GRAY_100;
/// The side panels' background (`panel_bg_color`, Gray.100).
pub const PANEL_BG: Color32 = GRAY_100;
/// The timeline's and the status bar's background (`bottom_bar_color`, Gray.150).
pub const BOTTOM_BAR_BG: Color32 = GRAY_150;
/// Menus', dialogs', and the navigation bar's background (`floating_color`, Gray.200).
pub const FLOATING_BG: Color32 = GRAY_200;
/// A text field's background (`text_edit_bg_color`, Gray.200).
pub const FIELD_BG: Color32 = GRAY_200;
/// A hovered row's background (`list_item_hovered_bg`, Gray.250).
pub const HOVER_BG: Color32 = GRAY_250;
/// The 1 px border between panels, and separators (`widget_noninteractive_bg_stroke`,
/// Gray.250).
pub const PANEL_BORDER: Color32 = GRAY_250;
/// A button's background at rest, and a toolbar button's hovered
/// (`widget_inactive_bg_fill`, Gray.300).
pub const WIDGET_BG: Color32 = GRAY_300;
/// A secondary button's background hovered (`button_secondary`, Gray.350).
pub const WIDGET_HOVER_BG: Color32 = GRAY_350;
/// Key caps' and glyphs' borders, and a field's at rest (`form_selectable_stroke`,
/// Gray.350).
pub const STROKE_STRONG: Color32 = GRAY_350;
/// What dims the window behind a modal (`modal_backdrop_color`, `#000000CC`).
pub const MODAL_BACKDROP: Color32 = hex_alpha(0x000000, 0xcc);

// ---- Text ----

/// Labels, hints, units, and section headers (`text_subdued`, Gray.550).
pub const TEXT_SUBDUED: Color32 = GRAY_550;
/// A browser row's name (`list_item_default_text`, Gray.700).
pub const TEXT_ROW: Color32 = GRAY_700;
/// Body text (`text_default`, Gray.750).
pub const TEXT_DEFAULT: Color32 = GRAY_750;
/// Titles, and values that matter (`text_strong`, Gray.1000).
pub const TEXT_STRONG: Color32 = GRAY_1000;

// ---- Selection and focus ----

/// A selected row's, the active tool's, and the selection's fill (`selection_bg_fill`,
/// Blue.500).
pub const SELECTION_BG: Color32 = hex(0x186add);
/// Text on [`SELECTION_BG`] (`selection_stroke_color`, `text_color_on_primary`, Blue.900).
pub const ON_SELECTION: Color32 = hex(0xf0f2ff);
/// The keyboard focus ring (Blue.525; approved over `re_ui`'s Blue.400, `#004bc2`, which is
/// about 2.5:1 on black).
pub const FOCUS: Color32 = hex(0x4676ff);
/// The focus ring's width.
pub const FOCUS_WIDTH: f32 = 1.0;
/// The halo outside the focus ring: Blue.525 at 25% (the prototype's `:focus-visible`).
pub const FOCUS_HALO: Color32 = hex_alpha(0x4676ff, 64);
/// The halo's width.
pub const FOCUS_HALO_WIDTH: f32 = 2.0;

// ---- States ----

/// An error's badges and marks, filled (`error_fg_color`, `#ab0116`).
pub const ERROR_FILL: Color32 = hex(0xab0116);
/// Error text and icons (Red.550; approved, since `#ab0116` is too dark for text on black).
pub const ERROR_TEXT: Color32 = hex(0xff3865);
/// An error card's fill (`alert_error`: Red.500 at 18%).
pub const ALERT_ERROR_BG: Color32 = hex_alpha(0xec004f, 46);
/// A blocked feature's mark, and warnings (`warn_fg_color`, `#ff7a0c`).
pub const WARNING: Color32 = hex(0xff7a0c);
/// A warning banner's fill (`alert_warning`: `#df7700` at 18%).
pub const ALERT_WARNING_BG: Color32 = hex_alpha(0xdf7700, 46);
/// A warning banner's text (`alert_warning`).
pub const ALERT_WARNING_TEXT: Color32 = hex(0xffd28a);
/// "Rebuilt" and ready dots (`success_text_color`, Green.700).
pub const SUCCESS: Color32 = hex(0x00da7e);
/// What an agent or another program changed on disk (Purple.600; approved, `re_ui` has no
/// such token).
pub const CHANGED_OUTSIDE: Color32 = hex(0xb780ff);

// ---- Buttons ----

/// The primary button's fill: OK, Create, New part (`button_primary`, Gray.800).
pub const PRIMARY_BUTTON_BG: Color32 = GRAY_800;
/// The primary button's fill hovered (Gray.850).
pub const PRIMARY_BUTTON_HOVER_BG: Color32 = GRAY_850;
/// The primary button's text (Gray.250).
pub const PRIMARY_BUTTON_TEXT: Color32 = GRAY_250;

// ---- Axes: the grid's, the triad's, and the view cube's stubs (`axis_color_x/y/z`) ----

/// X (Red.600).
pub const AXIS_X: Rgba8 = rgba8(0xff6981);
/// Y (Green.600).
pub const AXIS_Y: Rgba8 = rgba8(0x00b266);
/// Z (Blue.600).
pub const AXIS_Z: Rgba8 = rgba8(0x7a95ff);

// ---- Sketches: definition states as SolidWorks means them, in the dark theme's colours ----

/// Under defined (Blue.600).
pub const SKETCH_UNDER_DEFINED: Color32 = hex(0x7a95ff);
/// Fully defined (Gray.900: SolidWorks' black on a dark canvas).
pub const SKETCH_FULLY_DEFINED: Color32 = hex(0xeeeeed);
/// Over defined, and unsolvable ([`ERROR_TEXT`]'s red).
pub const SKETCH_OVER_DEFINED: Color32 = hex(0xff3865);
/// Invalid geometry (approved; `re_ui` has no yellow).
pub const SKETCH_INVALID: Color32 = hex(0xf2c94c);
/// Dangling (approved).
pub const SKETCH_DANGLING: Color32 = hex(0xb5b84a);
/// A driven dimension (Gray.500).
pub const SKETCH_DRIVEN: Color32 = hex(0x827f7f);
/// A curve's stroke.
pub const SKETCH_STROKE_WIDTH: f32 = 1.8;
/// Construction geometry: dashed in its state's colour, this long a dash and this long a
/// gap, in points.
pub const SKETCH_CONSTRUCTION_DASH: [f32; 2] = [5.0, 3.0];
/// A selected (or hovered) entity's stroke, over its halo (approved: a halo, not
/// SolidWorks' green, so it reads apart from under-defined blue).
pub const SKETCH_SELECTED_STROKE: Color32 = GRAY_1000;
/// That stroke's width.
pub const SKETCH_SELECTED_WIDTH: f32 = 2.4;
/// The halo under a selected entity: [`FOCUS`]'s blue at 35%.
pub const SKETCH_SELECTED_HALO: Color32 = hex_alpha(0x4676ff, 89);
/// The halo's width, centred on the curve.
pub const SKETCH_SELECTED_HALO_WIDTH: f32 = 6.0;
/// A sketch point's square's side.
pub const SKETCH_POINT_SIZE: f32 = 5.0;
/// A closed profile's fill: Blue.500 at 10%.
pub const SKETCH_PROFILE_FILL: Color32 = hex_alpha(0x186add, 26);
/// A dimension's text's size.
pub const CANVAS_TEXT_SIZE: f32 = 11.5;
/// Other text drawn on the canvas: a length while drawing, the view's name.
pub const CANVAS_LABEL_SIZE: f32 = 11.0;

// ---- The viewport: plain values the app passes to the renderer's frames ----

/// The canvas (`viewport_background`, Gray.0).
pub const VIEWPORT_BG: Rgba8 = rgba8(0x000000);
/// A body's colour.
pub const VIEWPORT_BODY: Rgba8 = rgba8(0xb9c0ca);
/// A body's edges.
pub const VIEWPORT_EDGE: Rgba8 = rgba8(0x101010);
/// Their width, in points.
pub const VIEWPORT_EDGE_WIDTH: f32 = 1.5;
/// A hovered face's tint: white, mixed in at [`VIEWPORT_HOVER_TINT_AMOUNT`].
pub const VIEWPORT_HOVER_TINT: Rgba8 = rgba8(0xffffff);
/// How much of [`VIEWPORT_HOVER_TINT`] a hovered face takes.
pub const VIEWPORT_HOVER_TINT_AMOUNT: f32 = 0.28;
/// A hovered edge.
pub const VIEWPORT_HOVER_EDGE: Rgba8 = rgba8(0xffffff);
/// A hovered edge's width.
pub const VIEWPORT_HOVER_EDGE_WIDTH: f32 = 3.0;
/// A selected face's tint, mixed in at [`VIEWPORT_SELECTION_TINT_AMOUNT`]: the prototype's
/// blue, a little brighter than [`SELECTION_BG`] so it reads on a lit grey body.
pub const VIEWPORT_SELECTION_TINT: Rgba8 = rgba8(0x2f7bff);
/// How much of [`VIEWPORT_SELECTION_TINT`] a selected face takes.
pub const VIEWPORT_SELECTION_TINT_AMOUNT: f32 = 0.55;
/// A selected edge.
pub const VIEWPORT_SELECTION_EDGE: Rgba8 = rgba8(0x5b95ff);
/// A selected edge's width.
pub const VIEWPORT_SELECTION_EDGE_WIDTH: f32 = 3.5;
/// The faces a feature made, while it's hovered in the timeline: Blue.800, mixed in at
/// [`VIEWPORT_FEATURE_HOVER_TINT_AMOUNT`].
pub const VIEWPORT_FEATURE_HOVER_TINT: Rgba8 = rgba8(0xd4dbff);
/// How much of [`VIEWPORT_FEATURE_HOVER_TINT`] those faces take.
pub const VIEWPORT_FEATURE_HOVER_TINT_AMOUNT: f32 = 0.4;
/// A broken reference's candidates (`warn_fg_color`).
pub const VIEWPORT_CANDIDATE: Rgba8 = rgba8(0xff7a0c);
/// A candidate edge's width.
pub const VIEWPORT_CANDIDATE_WIDTH: f32 = 2.5;
/// The hovered candidate's width.
pub const VIEWPORT_CANDIDATE_HOT_WIDTH: f32 = 4.0;
/// The grid's minor lines: `#3a3939` at 75%.
pub const VIEWPORT_GRID_MINOR: Rgba8 = [0x3a, 0x39, 0x39, 191];
/// The grid's major lines: Gray.400 at 90%.
pub const VIEWPORT_GRID_MAJOR: Rgba8 = [0x64, 0x63, 0x63, 230];
/// The minor lines' spacing, in millimetres.
pub const VIEWPORT_GRID_MINOR_MM: f32 = 10.0;
/// The major lines' spacing, in millimetres.
pub const VIEWPORT_GRID_MAJOR_MM: f32 = 50.0;
/// How far from the origin the grid has faded out, in millimetres: linearly, from full at
/// the origin.
pub const VIEWPORT_GRID_FADE_MM: f32 = 200.0;
/// The grid's axis lines' width, each in its axis's colour ([`AXIS_X`], [`AXIS_Y`]).
pub const VIEWPORT_GRID_AXIS_WIDTH: f32 = 1.5;
/// A box select's fill, window or crossing: Blue.500 at 14%.
pub const BOX_SELECT_FILL: Rgba8 = [0x18, 0x6a, 0xdd, 36];
/// A box select's outline ([`FOCUS`]'s blue): solid for a window (left to right), dashed
/// for a crossing (right to left).
pub const BOX_SELECT_STROKE: Rgba8 = rgba8(0x4676ff);
/// The outline's width.
pub const BOX_SELECT_STROKE_WIDTH: f32 = 1.0;
/// A crossing box's dash and gap, in points.
pub const BOX_SELECT_CROSSING_DASH: [f32; 2] = [4.0, 3.0];

// ---- Spacing (DESIGN.md, *Spacing*; `re_ui`'s tokens) ----

/// Margin on every side of a view, and a panel's padding (`view_padding`).
pub const VIEW_PADDING: i8 = 12;

/// A panel's margin: the view's padding at its sides (`panel_margin`).
pub const PANEL_MARGIN: Margin = Margin::symmetric(VIEW_PADDING, 0);

/// The space between items (`item_spacing`).
pub const ITEM_SPACING: f32 = 8.0;

/// The application bar's height: `re_ui`'s 28, and 2 for macOS's traffic lights.
pub const TOP_BAR_HEIGHT: f32 = 30.0;

/// A pane's title row, and a tab bar's (`title_bar_height`).
pub const TITLE_BAR_HEIGHT: f32 = 24.0;

/// A row of the browser, or an entry of the timeline (`list_item_height`).
pub const LIST_ITEM_HEIGHT: f32 = 24.0;

/// A tree's indent: a small icon, 14, and the space after it, 4.
pub const TREE_INDENT: f32 = 18.0;

/// The toolbar's row of tabs (DESIGN, SOLID, SKETCH).
pub const TOOLBAR_TABS_HEIGHT: f32 = 24.0;

/// The toolbar's row of panels (CREATE, MODIFY, ...), under its tabs.
pub const TOOLBAR_PANELS_HEIGHT: f32 = 52.0;

/// A toolbar button's side; its icon is `icons::TOOLBAR_ICON_SIZE`.
pub const TOOLBAR_BUTTON_SIZE: f32 = 30.0;

/// The timeline's height.
pub const TIMELINE_HEIGHT: f32 = 40.0;

/// The status bar's height.
pub const STATUS_BAR_HEIGHT: f32 = 24.0;

/// A text field's height.
pub const FIELD_HEIGHT: f32 = 22.0;

/// A key cap's height.
pub const KEY_CAP_HEIGHT: f32 = 16.0;

/// Windows' corners (`window_corner_radius`).
pub const WINDOW_CORNER_RADIUS: u8 = 6;

/// Buttons' and frames' corners (`normal_corner_radius`).
pub const NORMAL_CORNER_RADIUS: u8 = 6;

/// Small controls' corners (`small_corner_radius`).
pub const SMALL_CORNER_RADIUS: u8 = 4;

/// The ViewCube's side, and its margin from the canvas's top right; the Home button sits
/// above it, this high. Silico's own: Fusion publishes no sizes (docs/ux/fusion.md).
pub const VIEW_CUBE_SIDE: f32 = 96.0;
pub const VIEW_CUBE_MARGIN: f32 = 8.0;
pub const VIEW_CUBE_HOME_HEIGHT: f32 = 20.0;

/// The navigation bar's width and height, and its margin from the canvas's bottom.
pub const NAVIGATION_BAR_WIDTH: f32 = 220.0;
pub const NAVIGATION_BAR_HEIGHT: f32 = 26.0;
pub const NAVIGATION_BAR_MARGIN: f32 = 8.0;

// ---- Type (DESIGN.md, *Type*) ----

/// Body text: 12 points (`re_ui`'s Body), on a 16 point line where a caller sets one.
pub const BODY_SIZE: f32 = 12.0;
/// Body text's line height, for a caller's `RichText::line_height` (egui's style has none).
pub const BODY_LINE_HEIGHT: f32 = 16.0;
/// Section headers, uppercase, and small text.
pub const HEADER_SIZE: f32 = 11.0;
/// Titles.
pub const TITLE_SIZE: f32 = 13.0;
/// IDs and element names, in [`MONOSPACE_FONT`].
pub const MONOSPACE_SIZE: f32 = 11.0;

/// The text font's name, as the fonts list it: Inter Medium, the one weight (egui draws no
/// synthetic bold, so emphasis is colour).
pub const FONT: &str = "Inter";

/// The monospace font's name: JetBrains Mono Regular, for IDs and element names.
pub const MONOSPACE_FONT: &str = "JetBrains Mono";

/// The fonts the app draws with: Inter for text, and JetBrains Mono for monospace, Inter
/// behind it for anything it lacks; egui's default fonts aren't built in.
pub fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        FONT.to_owned(),
        Arc::new(FontData::from_static(silico_font::INTER_MEDIUM)),
    );
    fonts.font_data.insert(
        MONOSPACE_FONT.to_owned(),
        Arc::new(FontData::from_static(silico_font::JETBRAINS_MONO_REGULAR)),
    );
    fonts
        .families
        .insert(FontFamily::Proportional, vec![FONT.to_owned()]);
    fonts.families.insert(
        FontFamily::Monospace,
        vec![MONOSPACE_FONT.to_owned(), FONT.to_owned()],
    );
    fonts
}

/// The app's look applied to `ctx`: its fonts, the dark theme always, its style from these
/// tokens, and the image loaders its icons need (`icons`).
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(ThemePreference::Dark);
    ctx.set_style_of(Theme::Dark, style(egui::Style::default()));
    egui_extras::install_image_loaders(ctx);
}

/// `base` with the tokens set: the app's style.
///
/// egui draws a focused button with its `active` look, so that look's outline is the
/// focus ring; a focused text field is outlined in `selection.stroke`, which is also the
/// text on a selection, so it's [`ON_SELECTION`] there, and a field drawn in
/// [`field_frame`] shows the ring itself. egui draws no halo outside a focus ring.
pub fn style(mut base: egui::Style) -> egui::Style {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL_BG;
    visuals.window_fill = FLOATING_BG;
    visuals.faint_bg_color = BOTTOM_BAR_BG;
    visuals.extreme_bg_color = color32(VIEWPORT_BG);
    visuals.text_edit_bg_color = Some(FIELD_BG);
    visuals.code_bg_color = FIELD_BG;
    visuals.weak_text_color = Some(TEXT_SUBDUED);
    visuals.error_fg_color = ERROR_TEXT;
    visuals.warn_fg_color = WARNING;
    visuals.selection.bg_fill = SELECTION_BG;
    visuals.selection.stroke = Stroke::new(1.0, ON_SELECTION);
    visuals.window_corner_radius = CornerRadius::same(WINDOW_CORNER_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(WINDOW_CORNER_RADIUS);
    visuals.window_stroke = Stroke::new(1.0, PANEL_BORDER);
    visuals.indent_has_left_vline = false;
    visuals.striped = false;
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL_BG;
    widgets.noninteractive.weak_bg_fill = PANEL_BG;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, PANEL_BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DEFAULT);
    widgets.noninteractive.corner_radius = CornerRadius::same(SMALL_CORNER_RADIUS);
    for (state, fill, text, outline) in [
        (&mut widgets.inactive, WIDGET_BG, TEXT_DEFAULT, Stroke::NONE),
        (
            &mut widgets.hovered,
            WIDGET_HOVER_BG,
            TEXT_STRONG,
            Stroke::NONE,
        ),
        (
            &mut widgets.active,
            WIDGET_HOVER_BG,
            TEXT_STRONG,
            Stroke::new(FOCUS_WIDTH, FOCUS),
        ),
        (&mut widgets.open, WIDGET_BG, TEXT_DEFAULT, Stroke::NONE),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = Stroke::new(1.0, text);
        state.bg_stroke = outline;
        state.corner_radius = CornerRadius::same(SMALL_CORNER_RADIUS);
    }
    base.visuals = visuals;
    for (text, size, family) in [
        (TextStyle::Body, BODY_SIZE, FontFamily::Proportional),
        (TextStyle::Button, BODY_SIZE, FontFamily::Proportional),
        (TextStyle::Small, HEADER_SIZE, FontFamily::Proportional),
        (TextStyle::Heading, TITLE_SIZE, FontFamily::Proportional),
        (TextStyle::Monospace, MONOSPACE_SIZE, FontFamily::Monospace),
    ] {
        base.text_styles.insert(text, FontId::new(size, family));
    }
    base.spacing.item_spacing = Vec2::splat(ITEM_SPACING);
    base.spacing.indent = TREE_INDENT;
    base.spacing.interact_size.y = LIST_ITEM_HEIGHT - 4.0;
    base.spacing.window_margin = Margin::same(VIEW_PADDING);
    base.spacing.menu_margin = Margin::same(VIEW_PADDING);
    base
}

/// A text field's frame, as the Modify box draws one: [`FIELD_BG`], a 1 px
/// [`STROKE_STRONG`] outline, [`FOCUS`]'s when `focused`, and [`ERROR_TEXT`]'s when
/// `invalid` (whether focused or not: a value that doesn't parse shows while it's typed).
/// For a `TextEdit` with its own frame off (`.frame(false)`), whose text a caller colours
/// [`ERROR_TEXT`] too when invalid.
pub fn field_frame(focused: bool, invalid: bool) -> egui::Frame {
    let outline = if invalid {
        ERROR_TEXT
    } else if focused {
        FOCUS
    } else {
        STROKE_STRONG
    };
    egui::Frame::new()
        .fill(FIELD_BG)
        .stroke(Stroke::new(FOCUS_WIDTH, outline))
        .corner_radius(CornerRadius::same(SMALL_CORNER_RADIUS))
        .inner_margin(Margin::symmetric(6, 2))
        .shadow(Shadow::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inter_is_the_text_font_and_jetbrains_mono_the_monospace() {
        let fonts = fonts();
        assert_eq!(
            fonts.font_data.keys().collect::<Vec<_>>(),
            [&FONT.to_owned(), &MONOSPACE_FONT.to_owned()]
        );
        assert_eq!(fonts.families[&FontFamily::Proportional], ["Inter"]);
        assert_eq!(
            fonts.families[&FontFamily::Monospace],
            ["JetBrains Mono", "Inter"]
        );
        assert_eq!(fonts.families.len(), 2);
    }

    /// The prototype's values (docs/design/m5-ui/DESIGN.md, *Tokens*), typed from its
    /// table, not from the constants above.
    #[test]
    fn apply_sets_the_tokens_on_egui() {
        let ctx = egui::Context::default();
        apply(&ctx);
        assert_eq!(ctx.theme(), Theme::Dark);
        let style = ctx.global_style();
        let v = &style.visuals;
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, Color32::from_rgb(0x0d, 0x0d, 0x0d));
        assert_eq!(v.window_fill, Color32::from_rgb(0x21, 0x21, 0x21));
        assert_eq!(v.faint_bg_color, Color32::from_rgb(0x17, 0x17, 0x17));
        assert_eq!(v.extreme_bg_color, Color32::from_rgb(0, 0, 0));
        assert_eq!(
            v.text_edit_bg_color,
            Some(Color32::from_rgb(0x21, 0x21, 0x21))
        );
        assert_eq!(v.selection.bg_fill, Color32::from_rgb(0x18, 0x6a, 0xdd));
        assert_eq!(
            v.selection.stroke,
            Stroke::new(1.0, Color32::from_rgb(0xf0, 0xf2, 0xff))
        );
        // The focus ring: a focused widget is drawn in its active look.
        assert_eq!(
            v.widgets.active.bg_stroke,
            Stroke::new(1.0, Color32::from_rgb(0x46, 0x76, 0xff))
        );
        // Error text is Red.550, warnings re_ui's orange.
        assert_eq!(v.error_fg_color, Color32::from_rgb(0xff, 0x38, 0x65));
        assert_eq!(v.warn_fg_color, Color32::from_rgb(0xff, 0x7a, 0x0c));
        assert_eq!(v.weak_text_color(), Color32::from_rgb(0x93, 0x90, 0x90));
        assert_eq!(v.text_color(), Color32::from_rgb(0xcf, 0xcf, 0xcf));
        assert_eq!(
            v.widgets.inactive.bg_fill,
            Color32::from_rgb(0x2c, 0x2b, 0x2b)
        );
        assert_eq!(
            v.widgets.noninteractive.bg_stroke,
            Stroke::new(1.0, Color32::from_rgb(0x27, 0x26, 0x26))
        );
        assert_eq!(v.window_corner_radius, CornerRadius::same(6));
        assert_eq!(v.widgets.inactive.corner_radius, CornerRadius::same(4));
        // Spacing and type.
        assert_eq!(style.spacing.item_spacing, Vec2::splat(8.0));
        assert_eq!(style.spacing.indent, 18.0);
        assert_eq!(style.spacing.window_margin, Margin::same(12));
        for (text, size, family) in [
            (TextStyle::Body, 12.0, FontFamily::Proportional),
            (TextStyle::Button, 12.0, FontFamily::Proportional),
            (TextStyle::Small, 11.0, FontFamily::Proportional),
            (TextStyle::Heading, 13.0, FontFamily::Proportional),
            (TextStyle::Monospace, 11.0, FontFamily::Monospace),
        ] {
            assert_eq!(
                style.text_styles[&text],
                FontId::new(size, family),
                "{text:?}"
            );
        }
        // The icons' loader.
        assert!(ctx.is_loader_installed(egui_extras::loaders::svg_loader::SvgLoader::ID));
    }

    #[test]
    fn apply_keeps_the_dark_theme_whatever_the_system_says() {
        let ctx = egui::Context::default();
        apply(&ctx);
        ctx.options_mut(|o| o.fallback_theme = Theme::Light);
        ctx.input_mut(|i| i.raw.system_theme = Some(Theme::Light));
        assert_eq!(ctx.theme(), Theme::Dark);
    }

    #[test]
    fn a_fields_outline_is_red_when_invalid_blue_when_focused_and_grey_at_rest() {
        let red = Color32::from_rgb(0xff, 0x38, 0x65);
        for (focused, invalid, outline) in [
            (false, false, Color32::from_rgb(0x52, 0x51, 0x51)),
            (true, false, Color32::from_rgb(0x46, 0x76, 0xff)),
            (false, true, red),
            (true, true, red),
        ] {
            let frame = field_frame(focused, invalid);
            assert_eq!(
                frame.stroke,
                Stroke::new(1.0, outline),
                "{focused} {invalid}"
            );
            assert_eq!(frame.fill, Color32::from_rgb(0x21, 0x21, 0x21));
            assert_eq!(frame.corner_radius, CornerRadius::same(4));
        }
    }

    #[test]
    fn the_translucent_tokens_are_their_fractions_of_255() {
        // DESIGN.md's and styles.css's percentages, rounded to the nearest of 255 steps.
        for (color, alpha) in [
            (FOCUS_HALO, 0.25),
            (ALERT_ERROR_BG, 0.18),
            (ALERT_WARNING_BG, 0.18),
            (SKETCH_SELECTED_HALO, 0.35),
            (SKETCH_PROFILE_FILL, 0.10),
            (color32(BOX_SELECT_FILL), 0.14),
            (color32(VIEWPORT_GRID_MINOR), 0.75),
            (color32(VIEWPORT_GRID_MAJOR), 0.90),
            (MODAL_BACKDROP, 0.8),
        ] {
            assert_eq!(color.a(), (alpha * 255.0f32).round() as u8, "{color:?}");
        }
    }
}
