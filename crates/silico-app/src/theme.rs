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

/// `0xrrggbb` with `alpha` out of 255, not premultiplied.
const fn hex_alpha(rgb: u32, alpha: u8) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color32::from_rgba_unmultiplied_const(r, g, b, alpha)
}

/// An opaque `color` at `alpha` out of 255, not premultiplied.
const fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(color.r(), color.g(), color.b(), alpha)
}

/// An opaque `color` as the renderer takes it, at `alpha` out of 255.
const fn rgba8_of(color: Color32, alpha: u8) -> Rgba8 {
    [color.r(), color.g(), color.b(), alpha]
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
/// Gray.600.
pub const GRAY_600: Color32 = hex(0x9f9f9f);
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
/// A dialog's field hovered (the prototype's `.field:hover`, Gray.250).
pub const FIELD_HOVER_BG: Color32 = GRAY_250;
/// The Modify box's field (the prototype's `.modify-box input`, Gray.150).
pub const MODIFY_FIELD_BG: Color32 = GRAY_150;
/// A hovered row's background (`list_item_hovered_bg`, Gray.250).
pub const HOVER_BG: Color32 = GRAY_250;
/// The 1 px border between panels, and separators (`widget_noninteractive_bg_stroke`,
/// Gray.250), [`BORDER_WIDTH`] wide.
pub const PANEL_BORDER: Color32 = GRAY_250;
/// A border's and a separator's width (`border: 1px`).
pub const BORDER_WIDTH: f32 = 1.0;
/// A button's background at rest, and a toolbar button's hovered
/// (`widget_inactive_bg_fill`, Gray.300).
pub const WIDGET_BG: Color32 = GRAY_300;
/// A button's background hovered (the prototype's `.btn-secondary:hover`, Gray.350).
pub const WIDGET_HOVER_BG: Color32 = GRAY_350;
/// Key caps' and glyphs' borders, and a field's at rest (`form_selectable_stroke`,
/// Gray.350).
pub const STROKE_STRONG: Color32 = GRAY_350;
/// What dims the window behind a modal: black at 60% (the prototype's `.backdrop`;
/// `re_ui`'s `modal_backdrop_color` is `#000000CC`).
pub const MODAL_BACKDROP: Color32 = with_alpha(GRAY_0, 153);

// ---- Text ----

/// Labels, hints, units, and section headers (`text_subdued`, Gray.550).
pub const TEXT_SUBDUED: Color32 = GRAY_550;
/// Disabled text and icons (`text_readonly`, Gray.400: the prototype's `.ab-btn:disabled`,
/// `.tb-btn:disabled`, and `.menu .mi:disabled`).
pub const TEXT_DISABLED: Color32 = GRAY_400;
/// A browser row's icon (`list_item_default_icon`, Gray.500; the prototype's `.row .r-icon`).
pub const ROW_ICON: Color32 = GRAY_500;
/// A browser row's name (`list_item_default_text`, Gray.700).
pub const TEXT_ROW: Color32 = GRAY_700;
/// A hovered browser row's name (Gray.800; the prototype's `.row:hover`).
pub const TEXT_ROW_HOVER: Color32 = GRAY_800;
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
/// The halo outside the focus ring: [`FOCUS`] at 25% (the prototype's `:focus-visible`).
pub const FOCUS_HALO: Color32 = with_alpha(FOCUS, 64);
/// The halo's width.
pub const FOCUS_HALO_WIDTH: f32 = 2.0;

/// The focus ring round `rect`, with `corner_radius`, on its outline's `kind` side, if
/// `response` has the keyboard's focus.
pub fn focus_ring(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    corner_radius: u8,
    kind: egui::StrokeKind,
) {
    if response.has_focus() {
        ui.painter()
            .rect_stroke(rect, corner_radius, Stroke::new(FOCUS_WIDTH, FOCUS), kind);
    }
}

/// A key cap's box (`.kbd`) in `rect`: `fill` under its border in `border`,
/// [`KEY_CAP_BORDER`] at its top and sides and [`KEY_CAP_BORDER_BOTTOM`] at its bottom, its
/// outer corners [`KEY_CAP_CORNER_RADIUS`] round, as CSS draws a `border-radius` with sides of
/// two widths. The border is one mesh, so a translucent one (`.btn .kbd`'s) is laid once
/// everywhere, and its outer edge is on the pixel grid, as egui's rectangles' are.
pub fn key_cap_box(ui: &egui::Ui, rect: egui::Rect, fill: Color32, border: Color32) {
    use egui::emath::GuiRounding as _;
    let painter = ui.painter();
    painter.rect_filled(rect, KEY_CAP_CORNER_RADIUS, fill);
    let pixels = ui.ctx().pixels_per_point();
    let feather = ui.ctx().tessellation_options(|options| {
        if options.feathering {
            options.feathering_size_in_pixels / pixels
        } else {
            0.0
        }
    });
    painter.add(key_cap_border(
        rect.round_to_pixels(pixels),
        border,
        feather,
    ));
}

/// Where a key cap's words' middle goes in its box `rect`: across, its middle
/// (`text-align: center`, its side borders alike); down, its line's middle, its line starting
/// under its top border.
pub fn key_cap_text_centre(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        rect.center().x,
        rect.top() + KEY_CAP_BORDER + KEY_CAP_LINE_HEIGHT / 2.0,
    )
}

/// The ring `.kbd`'s border makes round `rect`, in `colour`, faded to nothing over `feather`
/// across each of its edges. Each outer corner is a quarter circle of the cap's radius; each
/// inner one, as CSS has it, a quarter ellipse whose radii are the outer's less the border's
/// widths at it (none if that's less than nothing), about the same centre while the radius is
/// wider than the border.
fn key_cap_border(rect: egui::Rect, colour: Color32, feather: f32) -> egui::Mesh {
    use egui::{Pos2, vec2};
    use std::f32::consts::{FRAC_PI_2, PI};
    /// Each quarter's segments.
    const STEPS: u16 = 8;
    let mut mesh = egui::Mesh::default();
    if !rect.is_positive() {
        return mesh;
    }
    let radius = f32::from(KEY_CAP_CORNER_RADIUS)
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0);
    // Each side's feather at most as wide as the thinnest side, so no edge's crosses another's.
    let half = feather.clamp(0.0, KEY_CAP_BORDER.min(KEY_CAP_BORDER_BOTTOM)) / 2.0;
    // Each corner clockwise from the top left: its point, the way into the cap from it, the
    // border's widths across it (its side's) and down it (its top's or bottom's), and the angle
    // its quarter starts at (y down).
    let corners: [(Pos2, egui::Vec2, f32, f32, f32); 4] = [
        (
            rect.left_top(),
            vec2(1.0, 1.0),
            KEY_CAP_BORDER,
            KEY_CAP_BORDER,
            PI,
        ),
        (
            rect.right_top(),
            vec2(-1.0, 1.0),
            KEY_CAP_BORDER,
            KEY_CAP_BORDER,
            1.5 * PI,
        ),
        (
            rect.right_bottom(),
            vec2(-1.0, -1.0),
            KEY_CAP_BORDER,
            KEY_CAP_BORDER_BOTTOM,
            0.0,
        ),
        (
            rect.left_bottom(),
            vec2(1.0, -1.0),
            KEY_CAP_BORDER,
            KEY_CAP_BORDER_BOTTOM,
            FRAC_PI_2,
        ),
    ];
    for (corner, inward, across, down, start) in corners {
        let outer_centre = corner + inward * radius;
        let (rx, ry) = ((radius - across).max(0.0), (radius - down).max(0.0));
        let inner_centre = corner + inward * vec2(across + rx, down + ry);
        for step in 0..=STEPS {
            let angle = start + FRAC_PI_2 * f32::from(step) / f32::from(STEPS);
            let way = vec2(libm::cosf(angle), libm::sinf(angle));
            let outer = outer_centre + way * radius;
            let inner = inner_centre + way * vec2(rx, ry);
            // The inner edge's normal, out of the cap's inside: the ellipse's gradient.
            let normal = if rx > 0.0 && ry > 0.0 {
                vec2(way.x / rx, way.y / ry).normalized()
            } else {
                way
            };
            mesh.colored_vertex(outer + way * half, Color32::TRANSPARENT);
            mesh.colored_vertex(outer - way * half, colour);
            mesh.colored_vertex(inner + normal * half, colour);
            mesh.colored_vertex(inner - normal * half, Color32::TRANSPARENT);
        }
    }
    // Each point's four to the next's: the outer feather, the border, the inner feather.
    let points = 4 * (u32::from(STEPS) + 1);
    for point in 0..points {
        let (this, next) = (4 * point, 4 * ((point + 1) % points));
        for band in 0..3 {
            mesh.add_triangle(this + band, this + band + 1, next + band);
            mesh.add_triangle(this + band + 1, next + band + 1, next + band);
        }
    }
    mesh
}

// ---- States ----

/// An error's badges and marks, filled (`error_fg_color`, `#ab0116`).
pub const ERROR_FILL: Color32 = hex(0xab0116);
/// Error text and icons (Red.550; approved, since `#ab0116` is too dark for text on black).
pub const ERROR_TEXT: Color32 = hex(0xff3865);
/// An error card's fill: Red.500 at 18%, the prototype's `--alert-red-bg` (`re_ui`'s
/// `alert_error` has it at alpha 50, 19.6%).
pub const ALERT_ERROR_BG: Color32 = hex_alpha(0xec004f, 46);
/// A blocked feature's mark, and warnings (`warn_fg_color`, `#ff7a0c`).
pub const WARNING: Color32 = hex(0xff7a0c);
/// A warning banner's fill: `#df7700` at 18%, the prototype's `--alert-warn-bg` (`re_ui`'s
/// `alert_warning` has it at alpha 50, 19.6%).
pub const ALERT_WARNING_BG: Color32 = hex_alpha(0xdf7700, 46);
/// A warning banner's text (`alert_warning`).
pub const ALERT_WARNING_TEXT: Color32 = hex(0xffd28a);
/// "Rebuilt" and ready dots (`success_text_color`, Green.700).
pub const SUCCESS: Color32 = hex(0x00da7e);
/// What an agent or another program changed on disk (the prototype's `--outside`): the info
/// tone, [`INFO`], light and unsaturated, apart from the selection's saturated Blue.500
/// (Trevin's change of 2026-11-09: the purple it was didn't fit the theme).
pub const CHANGED_OUTSIDE: Color32 = INFO;
/// Links and information (`info_text_color`, Blue.700; the prototype's `--info`).
pub const INFO: Color32 = hex(0xadbbff);

// ---- Buttons ----

/// The primary button's fill: OK, Create, New part (`button_primary`, Gray.800).
pub const PRIMARY_BUTTON_BG: Color32 = GRAY_800;
/// The primary button's fill hovered (Gray.850).
pub const PRIMARY_BUTTON_HOVER_BG: Color32 = GRAY_850;
/// The primary button's text (Gray.250).
pub const PRIMARY_BUTTON_TEXT: Color32 = GRAY_250;
/// The primary button's text hovered (the prototype's `.btn-primary:hover`, `#000`).
pub const PRIMARY_BUTTON_HOVER_TEXT: Color32 = GRAY_0;
/// The primary button's fill disabled (the prototype's `.btn-primary:disabled`, Gray.300).
pub const PRIMARY_BUTTON_DISABLED_BG: Color32 = GRAY_300;
/// The primary button's text disabled (Gray.500).
pub const PRIMARY_BUTTON_DISABLED_TEXT: Color32 = GRAY_500;
/// A button that destroys something: its fill (the prototype's `.btn-danger`; `re_ui` has
/// none).
pub const DANGER_BUTTON_BG: Color32 = hex(0x5c001a);
/// Its text.
pub const DANGER_BUTTON_TEXT: Color32 = hex(0xffd1d6);
/// Its fill hovered (`.btn-danger:hover`).
pub const DANGER_BUTTON_HOVER_BG: Color32 = hex(0x730022);
/// Its text hovered.
pub const DANGER_BUTTON_HOVER_TEXT: Color32 = GRAY_1000;

// ---- Axes: the grid's, the triad's, and the view cube's stubs (`axis_color_x/y/z`) ----

/// X (Red.600).
pub const AXIS_X: Rgba8 = rgba8(0xff6981);
/// Y (Green.600).
pub const AXIS_Y: Rgba8 = rgba8(0x00b266);
/// Z (Blue.600).
pub const AXIS_Z: Rgba8 = rgba8(0x7a95ff);

// ---- Sketches: definition states as SolidWorks means them, in the dark theme's colours ----

/// Under defined (Blue.600, [`AXIS_Z`]'s blue).
pub const SKETCH_UNDER_DEFINED: Color32 = color32(AXIS_Z);
/// Fully defined (Gray.900: SolidWorks' black on a dark canvas).
pub const SKETCH_FULLY_DEFINED: Color32 = GRAY_900;
/// Over defined, and unsolvable ([`ERROR_TEXT`]'s red).
pub const SKETCH_OVER_DEFINED: Color32 = ERROR_TEXT;
/// Invalid geometry (approved; `re_ui` has no yellow).
pub const SKETCH_INVALID: Color32 = hex(0xf2c94c);
/// Dangling (approved).
pub const SKETCH_DANGLING: Color32 = hex(0xb5b84a);
/// A driven dimension (Gray.500).
pub const SKETCH_DRIVEN: Color32 = GRAY_500;

/// The colour a sketch in `state` is drawn and worded in, wherever it shows (sketch mode's
/// status, a shown sketch's lines): DESIGN.md's *Sketch definition states*, under defined
/// [`SKETCH_UNDER_DEFINED`], fully defined [`SKETCH_FULLY_DEFINED`], over defined and
/// unsolvable [`SKETCH_OVER_DEFINED`], invalid [`SKETCH_INVALID`].
pub fn sketch_state_colour(state: silico_core::sketch_states::SketchState) -> Color32 {
    use silico_core::sketch_states::SketchState;
    match state {
        SketchState::UnderDefined => SKETCH_UNDER_DEFINED,
        SketchState::FullyDefined => SKETCH_FULLY_DEFINED,
        SketchState::OverDefined | SketchState::NoSolution => SKETCH_OVER_DEFINED,
        SketchState::Invalid => SKETCH_INVALID,
    }
}
/// A curve's stroke.
pub const SKETCH_STROKE_WIDTH: f32 = 1.8;
/// Construction geometry: dashed in its state's colour (DESIGN.md, *Tokens*), this long a
/// dash and this long a gap, in points. The prototype draws no construction curve: this is
/// the dash of the line being drawn (sketch.js:341), its one dashed sketch curve. The
/// palette's legend shows construction as a 2 px dashed `#9f9f9f`, a neutral key, since its
/// colour is its state's; so not that.
pub const SKETCH_CONSTRUCTION_DASH: [f32; 2] = [5.0, 3.0];
/// A selected (or hovered) entity's stroke, over its halo (approved: a halo, not
/// SolidWorks' green, so it reads apart from under-defined blue).
pub const SKETCH_SELECTED_STROKE: Color32 = GRAY_1000;
/// That stroke's width.
pub const SKETCH_SELECTED_WIDTH: f32 = 2.4;
/// The halo under a selected entity: [`FOCUS`]'s blue at 35%.
pub const SKETCH_SELECTED_HALO: Color32 = with_alpha(FOCUS, 89);
/// The halo's width, centred on the curve.
pub const SKETCH_SELECTED_HALO_WIDTH: f32 = 6.0;
/// A sketch point's square's side.
pub const SKETCH_POINT_SIZE: f32 = 5.0;
/// A closed profile's fill: [`SELECTION_BG`]'s Blue.500 at 10%.
pub const SKETCH_PROFILE_FILL: Color32 = with_alpha(SELECTION_BG, 26);
/// A dimension's text's size.
pub const CANVAS_TEXT_SIZE: f32 = 11.5;
/// Other text drawn on the canvas: a length while drawing, the view's name.
pub const CANVAS_LABEL_SIZE: f32 = 11.0;
/// A redundant relation's glyph, its entities untouched (docs/design/m5.0-app.md, section 9,
/// *Colours by state*). The prototype draws none; check reports one as a warning
/// (`constraints_redundant`), so it's [`WARNING`]'s orange.
pub const SKETCH_REDUNDANT: Color32 = WARNING;
/// A driving dimension's lines, arrows, and text (sketch.js:308, `#cfcfcf`); an over-defining
/// one is [`SKETCH_OVER_DEFINED`], and a driven one [`SKETCH_DRIVEN`], its value in brackets
/// (sketch.js:315 and :316).
pub const SKETCH_DIMENSION_TEXT: Color32 = GRAY_750;
/// How far a dimension's line sits from what it measures, in points (sketch.js:309); its
/// extension lines start [`SKETCH_DIMENSION_GAP`] from what it measures and run 5 past the
/// line (sketch.js:265).
pub const SKETCH_DIMENSION_OFFSET: f32 = 26.0;
/// The gap between what a dimension measures and its extension lines (sketch.js:265).
pub const SKETCH_DIMENSION_GAP: f32 = 4.0;
/// A dimension's arrowhead, filled: its length along the line and its width across it
/// (sketch.js:268).
pub const SKETCH_DIMENSION_ARROW: [f32; 2] = [7.0, 6.0];
/// The box under a dimension's value: black at 85%, its corners 3 (sketch.js:273).
pub const SKETCH_DIMENSION_LABEL_BG: Color32 = with_alpha(GRAY_0, 217);
/// That box's padding: 5 at its sides (sketch.js:272) and 18 high in all (sketch.js:273),
/// about 3 over and under [`CANVAS_TEXT_SIZE`] text.
pub const SKETCH_DIMENSION_LABEL_PADDING: Margin = Margin::symmetric(5, 3);
/// A relation's glyph: a square this side, its corners 3, its icon at 60% (sketch.js:260).
pub const SKETCH_GLYPH_SIZE: f32 = 16.0;
/// A glyph's fill (Gray.200, sketch.js:260).
pub const SKETCH_GLYPH_BG: Color32 = GRAY_200;
/// A glyph's border ([`STROKE_STRONG`], sketch.js:260); its icon is [`GRAY_700`]. A
/// relation the pointer shows takes [`FOCUS`]'s blue for both, a snap [`SKETCH_INVALID`]'s
/// yellow (sketch.js:345 and :346).
pub const SKETCH_GLYPH_STROKE: Color32 = STROKE_STRONG;
/// How far a glyph's centre sits from its curve, across it (sketch.js:302 and :333).
pub const SKETCH_GLYPH_OFFSET: f32 = 13.0;
/// The ring at the sketch's origin, 1.2 wide (sketch.js:254, `#cfcfcf`).
pub const SKETCH_ORIGIN_RING: Stroke = Stroke {
    width: 1.2,
    color: GRAY_750,
};
/// That ring's radius.
pub const SKETCH_ORIGIN_RING_RADIUS: f32 = 3.0;
/// The line being drawn, from the chain's last point to the pointer: under-defined blue,
/// 1.6 wide, dashed [`SKETCH_CONSTRUCTION_DASH`] (sketch.js:341).
pub const SKETCH_PREVIEW_STROKE: Stroke = Stroke {
    width: 1.6,
    color: SKETCH_UNDER_DEFINED,
};
/// The dotted guide to a point the pointer lines up with: [`FOCUS`]'s blue, 1 wide
/// (sketch.js:338).
pub const SKETCH_GUIDE_STROKE: Stroke = Stroke {
    width: 1.0,
    color: FOCUS,
};
/// That guide's dash and gap, in points (sketch.js:338).
pub const SKETCH_GUIDE_DASH: [f32; 2] = [2.0, 3.0];
/// The ring round a point the pointer snaps to, radius 6 (sketch.js:346, `#f2c94c`).
pub const SKETCH_SNAP_STROKE: Stroke = Stroke {
    width: 1.5,
    color: SKETCH_INVALID,
};
/// The pointer's crosshair while a tool draws: white, 1 wide, 8 each way (sketch.js:347).
pub const SKETCH_CROSSHAIR_STROKE: Stroke = Stroke {
    width: 1.0,
    color: GRAY_1000,
};
/// The Sketch Palette's width; it floats 12 from the canvas's right (styles.css, `.palette`).
pub const SKETCH_PALETTE_WIDTH: f32 = 212.0;
/// The sketch bar's height, at the canvas's top centre: a 22 high button, 4 of padding, and
/// a 1 border, each above and below (styles.css, `.sketch-bar`).
pub const SKETCH_BAR_HEIGHT: f32 = 32.0;

// ---- The viewport: plain values the app passes to the renderer's frames ----
/// How much of their colour the bodies keep while a sketch is open, in linear light
/// (`silico_render::render::Frame::dim`): the approved prototype dims the model to 0.45 of
/// its sRGB colour (sketch.js's `VP.setDim(0.45)`, viewport.js's `mix(vec3(0.0), c, uDim)`
/// on its output, three.js r149 writing sRGB values as they are), and the renderer works in
/// linear light. 0.18 is that dim worked by hand for the body's own colour, lit straight on:
/// `BODY` (170, 175, 182) in linear light (0.4020, 0.4287, 0.4678), 0.45 of it in sRGB,
/// (76.5, 78.75, 81.9), in linear light (0.0732, 0.0777, 0.0842), 0.182, 0.181, and 0.180
/// of it; 0.18 draws it (76.0, 78.5, 81.9). Edges and shaded faces take the same factor.
pub const SKETCH_MODEL_DIM: f32 = 0.18;

/// The canvas: a vertical gradient, soft charcoal along its top to a lighter grey along its
/// bottom, as Fusion's dark theme's canvas is (Trevin's change of 2026-11-09, DESIGN.md's
/// *Changes since approval*: a black canvas "looks like the void"), in place of `re_ui`'s
/// `viewport_background`, Gray.0, then darkened by him the same day once he saw it: its
/// first ends, #1e1f22 and #3a3c41, were "too light". Each end its own constant, to be set
/// from a screenshot.
pub const VIEWPORT_BG_TOP: Rgba8 = rgba8(0x141517);
pub const VIEWPORT_BG_BOTTOM: Rgba8 = rgba8(0x2a2c30);
/// The renderer's frames' own background: none, so they're composited over the canvas's
/// gradient, painted under them, which shows wherever no body is.
pub const VIEWPORT_FRAME_BG: Option<[u8; 3]> = None;
/// A body's colour (the prototype's; `re_ui` has none).
pub const VIEWPORT_BODY: Rgba8 = rgba8(0xb9c0ca);
/// A body's edges (the prototype's).
pub const VIEWPORT_EDGE: Rgba8 = rgba8(0x101010);
/// Their width, in points.
pub const VIEWPORT_EDGE_WIDTH: f32 = 1.5;
/// A shown sketch's curves drawn among the bodies (the prototype's `syncSketchLines`
/// lines), their width in points.
pub const VIEWPORT_SKETCH_WIDTH: f32 = 1.5;
/// An origin plane's fill (`crate::planes`, the prototype's `buildPlanes`): `#9f9f9f` at
/// 0.08.
pub const VIEWPORT_PLANE_FILL: Rgba8 = rgba8_of(GRAY_600, 20);
/// Its outline: `#9f9f9f` at 0.6.
pub const VIEWPORT_PLANE_OUTLINE: Rgba8 = rgba8_of(GRAY_600, 153);
/// Its outline's width, in points: the prototype's 1 px.
pub const VIEWPORT_PLANE_WIDTH: f32 = 1.0;
/// A hovered origin plane's fill (`VP.setPlanes`' hover): `#186add` at 0.32.
pub const VIEWPORT_PLANE_HOVER_FILL: Rgba8 = rgba8_of(SELECTION_BG, 82);
/// Its outline: `#4676ff` at 0.6.
pub const VIEWPORT_PLANE_HOVER_OUTLINE: Rgba8 = rgba8_of(FOCUS, 153);
/// A hovered face's tint: white, mixed in at [`VIEWPORT_HOVER_TINT_AMOUNT`].
pub const VIEWPORT_HOVER_TINT: Rgba8 = rgba8(0xffffff);
/// How much of [`VIEWPORT_HOVER_TINT`] a hovered face takes.
pub const VIEWPORT_HOVER_TINT_AMOUNT: f32 = 0.28;
/// A hovered edge.
pub const VIEWPORT_HOVER_EDGE: Rgba8 = rgba8(0xffffff);
/// A hovered edge's width.
pub const VIEWPORT_HOVER_EDGE_WIDTH: f32 = 3.0;
/// A selected face's tint, mixed in at [`VIEWPORT_SELECTION_TINT_AMOUNT`]: the prototype's
/// own blue, a little brighter than [`SELECTION_BG`] so it reads on a lit grey body.
pub const VIEWPORT_SELECTION_TINT: Rgba8 = rgba8(0x2f7bff);
/// How much of [`VIEWPORT_SELECTION_TINT`] a selected face takes.
pub const VIEWPORT_SELECTION_TINT_AMOUNT: f32 = 0.55;
/// A selected edge (the prototype's).
pub const VIEWPORT_SELECTION_EDGE: Rgba8 = rgba8(0x5b95ff);
/// A selected edge's width.
pub const VIEWPORT_SELECTION_EDGE_WIDTH: f32 = 3.5;
/// The faces a feature made, while it's hovered in the timeline: Blue.800, mixed into a
/// face's colour in linear light at [`VIEWPORT_FEATURE_HOVER_TINT_AMOUNT`], before it's lit
/// (`crate::viewport::feature_tint`).
pub const VIEWPORT_FEATURE_HOVER_TINT: Rgba8 = rgba8(0xd4dbff);
/// How much of [`VIEWPORT_FEATURE_HOVER_TINT`] those faces take.
pub const VIEWPORT_FEATURE_HOVER_TINT_AMOUNT: f32 = 0.4;
/// A hovered vertex's dot (interact.js, `overlay`'s `dot`).
pub const VIEWPORT_HOVER_VERTEX: Rgba8 = rgba8(0xffffff);
/// A selected vertex's dot.
pub const VIEWPORT_SELECTION_VERTEX: Rgba8 = rgba8(0x4676ff);
/// A vertex's dot's radius, in points.
pub const VIEWPORT_VERTEX_RADIUS: f32 = 4.5;
/// The width of the ring about a vertex's dot, in points.
pub const VIEWPORT_VERTEX_RING: f32 = 1.0;
/// The ring's colour.
pub const VIEWPORT_VERTEX_RING_COLOR: Rgba8 = rgba8(0x000000);
/// A broken reference's candidates ([`WARNING`], `warn_fg_color`).
pub const VIEWPORT_CANDIDATE: Rgba8 = rgba8_of(WARNING, 0xff);
/// A candidate edge's width.
pub const VIEWPORT_CANDIDATE_WIDTH: f32 = 2.5;
/// The hovered candidate's width.
pub const VIEWPORT_CANDIDATE_HOT_WIDTH: f32 = 4.0;
/// The grid's minor lines: `#3a3939` at 75% (the prototype's).
pub const VIEWPORT_GRID_MINOR: Rgba8 = [0x3a, 0x39, 0x39, 191];
/// The grid's major lines: [`GRAY_400`] at 90%.
pub const VIEWPORT_GRID_MAJOR: Rgba8 = rgba8_of(GRAY_400, 230);
/// The minor lines' spacing, in millimetres.
pub const VIEWPORT_GRID_MINOR_MM: f32 = 10.0;
/// The major lines' spacing, in millimetres.
pub const VIEWPORT_GRID_MAJOR_MM: f32 = 50.0;
/// How far from the origin the grid has faded out, in millimetres: linearly, from full at
/// the origin. The ground grid reaches as far ([`crate::grid`]).
pub const VIEWPORT_GRID_FADE_MM: f32 = 200.0;
/// The grid's minor and major lines' width, in points: a WebGL line's pixel at the
/// prototype's scale, as the sketch's grid draws them (`crate::sketch::draw`).
pub const VIEWPORT_GRID_WIDTH: f32 = 1.0;
/// The grid's axis lines' width, each in its axis's colour ([`AXIS_X`], [`AXIS_Y`], and
/// the ground grid's Z, [`AXIS_Z`]).
pub const VIEWPORT_GRID_AXIS_WIDTH: f32 = 1.5;
/// Their alpha, out of 255: 90% (viewport.js's `buildGrid`, its `axis`).
pub const VIEWPORT_GRID_AXIS_ALPHA: u8 = 230;
/// A box select's fill, window or crossing: [`SELECTION_BG`]'s Blue.500 at 14%.
pub const BOX_SELECT_FILL: Rgba8 = rgba8_of(SELECTION_BG, 36);
/// A box select's outline ([`FOCUS`]'s blue): solid for a window (left to right), dashed
/// for a crossing (right to left).
pub const BOX_SELECT_STROKE: Rgba8 = rgba8_of(FOCUS, 0xff);
/// The outline's width.
pub const BOX_SELECT_STROKE_WIDTH: f32 = 1.0;
/// A crossing box's dash and gap, in points.
pub const BOX_SELECT_CROSSING_DASH: [f32; 2] = [4.0, 3.0];

/// A toast (the prototype's `.toast`, `#toasts`: DESIGN.md's *Toasts*, an `Area` anchored
/// bottom-right of the canvas): Gray.200 inside a Gray.250 border, `--r`'s corners, a menu's
/// shadow (`0 6px 18px`, black at 60%), padded 7 over and under, 10 at the left and 8 at the
/// right, at most 420 wide, its text as the body's; 12 in from the canvas's right and 58 up
/// from its foot; shown for 3.8 seconds (app.js's `toast`). Several stack in a column, 6
/// apart, the newest last (`#toasts`' `gap: 6px`); each has its icon at the left, 8 from its
/// words (`.toast`'s `gap: 8px`), in the words' colour (`svg.i`'s `currentColor`). Its 420
/// counts its border and padding (`box-sizing: border-box`).
pub const TOAST_BG: Color32 = GRAY_200;
pub const TOAST_BORDER: Color32 = GRAY_250;
pub const TOAST_TEXT: Color32 = TEXT_DEFAULT;
pub const TOAST_ICON: Color32 = TOAST_TEXT;
pub const TOAST_GAP: f32 = 6.0;
pub const TOAST_ICON_GAP: f32 = 8.0;
pub const TOAST_CORNER_RADIUS: u8 = NORMAL_CORNER_RADIUS;
pub const TOAST_SHADOW: Shadow = Shadow {
    offset: [0, 6],
    blur: 18,
    spread: 0,
    color: with_alpha(GRAY_0, 153),
};
pub const TOAST_PADDING: Margin = Margin {
    left: 10,
    right: 8,
    top: 7,
    bottom: 7,
};
pub const TOAST_MAX_WIDTH: f32 = 420.0;
pub const TOAST_MARGIN_RIGHT: f32 = 12.0;
pub const TOAST_MARGIN_BOTTOM: f32 = 58.0;
pub const TOAST_SECONDS: f64 = 3.8;

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

/// A feature's rename field in the timeline (F2, or Rename in its menu), over its item: as
/// wide as the property panel's Name field shows a name, the prototype's `prompt` giving
/// none of its own; and the gap between it and the item.
pub const TIMELINE_RENAME_WIDTH: f32 = 180.0;
pub const TIMELINE_RENAME_GAP: f32 = 4.0;

/// A key cap's height.
pub const KEY_CAP_HEIGHT: f32 = 16.0;

/// Windows' corners (`window_corner_radius`).
pub const WINDOW_CORNER_RADIUS: u8 = 6;

/// Buttons' and frames' corners (`normal_corner_radius`).
pub const NORMAL_CORNER_RADIUS: u8 = 6;

/// Small controls' corners (`small_corner_radius`).
pub const SMALL_CORNER_RADIUS: u8 = 4;

/// A menu's padding (the prototype's `.menu`; `re_ui`'s is the view's 12).
pub const MENU_PADDING: i8 = 4;

/// A feature's menu as the prototype's `.menu` lays it out: 200 wide at least, its 1 border
/// and its padding inside that (`box-sizing: border-box`).
pub const MENU_MIN_WIDTH: f32 = 200.0;
/// A menu item (`.menu .mi`): 24 high, 8 of padding at each side, 8 between its icon (or
/// its 14 wide spacer), its words, and what's after them; its corners `--r-sm`.
pub const MENU_ITEM_HEIGHT: f32 = 24.0;
pub const MENU_ITEM_PADDING: f32 = 8.0;
pub const MENU_ITEM_GAP: f32 = 8.0;
/// An item's dim words after its label (the prototype's `subdued` span, `margin-left:
/// 8px; font-size: 11px`), as Edit feature's "double-click".
pub const MENU_ITEM_SUB_MARGIN: f32 = 8.0;
pub const MENU_ITEM_SUB_SIZE: f32 = 11.0;
pub const MENU_ITEM_SUB_TEXT: Color32 = TEXT_SUBDUED;
/// An item's key caps (`.menu .mi .sc`): at its right, 24 of padding at their left.
pub const MENU_KEYS_PADDING: f32 = 24.0;
/// An item's words (`.menu .mi`, Gray.750), hovered (`--blue`, `--blue-text`), and off
/// (`.menu .mi:disabled`, Gray.400).
pub const MENU_ITEM_TEXT: Color32 = TEXT_DEFAULT;
pub const MENU_ITEM_HOVER_BG: Color32 = SELECTION_BG;
pub const MENU_ITEM_HOVER_TEXT: Color32 = ON_SELECTION;
/// A hovered item's key caps (`.menu .mi:hover .kbd`): no fill, a white outline at 35%,
/// and the hovered item's words' colour.
pub const MENU_KEY_CAP_HOVER_STROKE: Color32 = hex_alpha(0xffffff, 89);
/// The menu's head (`.menu .mh`: `padding: 6px 8px 2px`, 10 high capitals on the body's
/// 16 line).
pub const MENU_HEAD_TOP: f32 = 6.0;
pub const MENU_HEAD_BOTTOM: f32 = 2.0;
pub const MENU_LINE_HEIGHT: f32 = 16.0;
/// Its separator (`.menu .msep`: 1 high, Gray.250, 4 above and below).
pub const MENU_SEPARATOR_MARGIN: f32 = 4.0;
pub const MENU_SEPARATOR: Color32 = GRAY_250;

/// Menus' and popups' shadow (the prototype's `.menu`: `0 8px 28px`, black at 60%).
pub const POPUP_SHADOW: Shadow = Shadow {
    offset: [0, 8],
    blur: 28,
    spread: 0,
    color: with_alpha(GRAY_0, 153),
};

// ---- Select Other's list (`.menu.select-other`; DESIGN.md's *Menus*, M5.7) ----
// Its frame and rows are a menu's, as the feature menus draw them ([`MENU_PADDING`],
// [`MENU_MIN_WIDTH`], [`MENU_ITEM_HEIGHT`] and the rest).

/// Select Other's foot (`.menu.select-other .foot`): 10.5 high, Gray.550, under a Gray.250
/// rule 4 below the rows, padded 4 over, 8 at the sides, 2 under, its keys as key caps.
pub const MENU_FOOT_TEXT: Color32 = TEXT_SUBDUED;
pub const MENU_FOOT_SIZE: f32 = 10.5;
pub const MENU_FOOT_RULE: Color32 = PANEL_BORDER;
pub const MENU_FOOT_MARGIN_TOP: f32 = 4.0;
pub const MENU_FOOT_PADDING: Margin = Margin {
    left: 8,
    right: 8,
    top: 4,
    bottom: 2,
};
/// How far right of and below its right click the canvas's context menu opens, in points:
/// so the click's own point stays the canvas's, and a right click there again opens another
/// there (the prototype's `onContext` opens one wherever the canvas is right-clicked).
pub const CONTEXT_MENU_GAP: f32 = 1.0;
/// Where Select Other's list opens from the right click that asked for it, in points: 28
/// right and 18 up (interact.js, `openSelectOther`'s `e.clientX + 28, e.clientY - 18`).
pub const SELECT_OTHER_OFFSET: [f32; 2] = [28.0, -18.0];
/// How many of Select Other's rows show at once: past them its list scrolls, so a crowd
/// under one pixel stays inside the window (the prototype lists every row; its scenes have
/// a few).
pub const SELECT_OTHER_ROWS: usize = 16;

/// A tooltip's padding: 6 over and under, 8 at the sides (the prototype's `.tooltip`). egui
/// draws its own tooltips in a menu's frame ([`MENU_PADDING`], [`NORMAL_CORNER_RADIUS`],
/// [`POPUP_SHADOW`]); these are for where the app draws one.
pub const TOOLTIP_PADDING: Margin = Margin::symmetric(8, 6);
/// A tooltip's corners (`.tooltip`).
pub const TOOLTIP_CORNER_RADIUS: u8 = SMALL_CORNER_RADIUS;
/// A tooltip's shadow (`.tooltip`: `0 6px 18px`, black at 60%).
pub const TOOLTIP_SHADOW: Shadow = Shadow {
    offset: [0, 6],
    blur: 18,
    spread: 0,
    color: with_alpha(GRAY_0, 153),
};

/// How long a view transition takes, in seconds (DESIGN.md's *egui mapping*: "a 200 ms view
/// transition"; the prototype's `animateTo` takes 220 ms by default, DESIGN.md the 200):
/// Home, Fit, a named view, a ViewCube click, and Look At ease to their view over it
/// (`crate::transition`). The app's own, not egui's `animation_time`, which only says
/// whether the view moves at all (0: it jumps).
pub const VIEW_TRANSITION_SECONDS: f64 = 0.2;

/// The ViewCube's side, and its margin from the canvas's top and right (the prototype's
/// viewport.js:101, `CUBE` and `CUBE_M`).
pub const VIEW_CUBE_SIDE: f32 = 92.0;
pub const VIEW_CUBE_MARGIN: f32 = 16.0;

/// The ViewCube's two buttons, Home view and its options' ▾ (app.js's `renderCanvasUI`, the
/// `home3d` and `cubeMenu` buttons): bar buttons (`.ab-btn`) in a column at the cube's left,
/// its right 4 left of the cube (`right: 16 + 92 + 4`) and its top level with the cube's
/// (`top: 16px`), 2 apart (`gap: 2px`), each filled Gray.200 at rest and hovered alike (an
/// inline `background`, over `.ab-btn:hover`'s).
pub const VIEW_CUBE_BUTTONS_GAP: f32 = 4.0;
pub const VIEW_CUBE_BUTTONS_SPACING: f32 = 2.0;
pub const VIEW_CUBE_BUTTON_BG: Color32 = GRAY_200;

/// Where the ViewCube's menu opens, from the ▾'s bottom left corner (interact.js's
/// `cubeMenu(r.left - 150, r.bottom + 2)`), and from a right click on the cube
/// (`onCubeMenu`'s `e.clientX - 160, e.clientY`): its top left corner there.
pub const VIEW_CUBE_MENU_FROM_BUTTON: Vec2 = Vec2::new(-150.0, 2.0);
pub const VIEW_CUBE_MENU_FROM_CLICK: Vec2 = Vec2::new(-160.0, 0.0);

/// The ViewCube's faces' fills (viewport.js's `buildCube`): the right and left alike
/// (Gray.300), the top lightest, the bottom darkest (Gray.200), the front and back between.
pub const VIEW_CUBE_SIDE_FILL: Color32 = GRAY_300;
pub const VIEW_CUBE_TOP_FILL: Color32 = hex(0x363535);
pub const VIEW_CUBE_BOTTOM_FILL: Color32 = GRAY_200;
pub const VIEW_CUBE_FRONT_FILL: Color32 = hex(0x303030);
pub const VIEW_CUBE_BACK_FILL: Color32 = hex(0x262626);
/// The border just inside each face's edge, and its width as a part of the face's side: the
/// prototype's face is a 128 px texture stroked 3 px wide at its rim in Gray.350
/// (viewport.js's `label`: `strokeRect(1.5, 1.5, 125, 125)`, `lineWidth = 3`).
pub const VIEW_CUBE_BORDER: Color32 = GRAY_350;
pub const VIEW_CUBE_BORDER_WIDTH: f32 = 3.0 / 128.0;
/// A face's label: its words in capitals in Gray.750, Inter Medium 21 px high on the
/// face's 128, given as a part of the face's side, and its middle 2 px of the 128 below the
/// face's (viewport.js's `label`: `fillText(text, 64, 66)`, its baseline `middle`).
pub const VIEW_CUBE_LABEL: Color32 = GRAY_750;
pub const VIEW_CUBE_LABEL_SIZE: f32 = 21.0 / 128.0;
pub const VIEW_CUBE_LABEL_DROP: f32 = 2.0 / 128.0;
/// The cube's edges, each drawn once over its faces: a WebGL line's 1 pixel, here 1 point,
/// in Gray.500 (viewport.js's `buildCube`, its `EdgesGeometry` in `#827f7f`).
pub const VIEW_CUBE_EDGE: Stroke = Stroke {
    width: 1.0,
    color: GRAY_500,
};
/// The cube's axis stubs at its corner, each in its axis's colour ([`AXIS_X`], [`AXIS_Y`],
/// [`AXIS_Z`]), a WebGL line's 1 pixel, here 1 point (viewport.js's `buildCube`, `ax`).
pub const VIEW_CUBE_STUB_WIDTH: f32 = 1.0;

/// The triad's origin, as far in from the canvas's left as up from its bottom
/// (viewport.js's `drawOverlay`: `ox = 44, oy = h - 44`).
pub const TRIAD_INSET: f32 = 44.0;
/// An axis's length on the screen square across the view, each shorter by how far it points
/// along it (`L = 26`).
pub const TRIAD_LENGTH: f32 = 26.0;
/// An axis's stroke, in its colour ([`AXIS_X`], [`AXIS_Y`], [`AXIS_Z`]), its ends round
/// (`stroke-width="2" stroke-linecap="round"`).
pub const TRIAD_WIDTH: f32 = 2.0;
/// How far past an axis's end its letter stands, along it (`L + 9`); how far below that the
/// letter's baseline is (`ly + 3.5`), the letter's middle across there (`text-anchor:
/// middle`); and its size (`font-size="10.5"`), in its axis's colour.
pub const TRIAD_LABEL_PAST: f32 = 9.0;
pub const TRIAD_LABEL_DROP: f32 = 3.5;
pub const TRIAD_LABEL_SIZE: f32 = 10.5;
/// The dot over the origin, drawn last: its radius and its fill (`<circle r="2"
/// fill="#cfcfcf"/>`, Gray.750).
pub const TRIAD_ORIGIN_RADIUS: f32 = 2.0;
pub const TRIAD_ORIGIN_FILL: Color32 = GRAY_750;

/// The view label beside the triad (styles.css's `.view-label`, at `left: 82px` as app.js's
/// `renderCanvasUI` places it): its line's left, in from the canvas's left; its line's
/// bottom, up from the canvas's bottom (`bottom: 12px`); and its line's height (the body's
/// 16).
pub const VIEW_LABEL_LEFT: f32 = 82.0;
pub const VIEW_LABEL_BOTTOM: f32 = 12.0;
pub const VIEW_LABEL_LINE_HEIGHT: f32 = BODY_LINE_HEIGHT;
/// Its words: [`CANVAS_LABEL_SIZE`] (`--fs-small`, 11) in Gray.500 (`--g500`).
pub const VIEW_LABEL_SIZE: f32 = CANVAS_LABEL_SIZE;
pub const VIEW_LABEL_TEXT: Color32 = GRAY_500;

/// A feature dialog's distance arrow in the canvas (interact.js's `arrowGeom` and its
/// overlay): its colour, Blue.525 (`#4676ff`, the focus ring's), and while it's dragged,
/// white (`dragArrow ? '#ffffff' : '#4676ff'`).
pub const ARROW: Color32 = FOCUS;
pub const ARROW_HELD: Color32 = GRAY_1000;
/// Its shaft: its width (`stroke-width="2.5"`), and its length on the screen from its base
/// along the sweep, whatever the zoom (`tip = a + u * 56`).
pub const ARROW_WIDTH: f32 = 2.5;
pub const ARROW_LENGTH: f32 = 56.0;
/// Its head, a triangle filled in its colour: its point this far past the shaft's end
/// (`g.ux * 12`), its corners this far either side of the shaft's end (`px * 6`).
pub const ARROW_HEAD_LENGTH: f32 = 12.0;
pub const ARROW_HEAD_HALF_WIDTH: f32 = 6.0;
/// The dot over its base, in its colour (`<circle r="3.5">`).
pub const ARROW_DOT_RADIUS: f32 = 3.5;
/// Its label, the value in mm: its box's left this far past the shaft's end along the arrow
/// (`g.ux * 22`), and this far right of that (`+ 6`), its middle down there; its height
/// (`height="20"`), its corners (`rx="4"`), and the room before its words and after them
/// (`lx + 6`, and the width's `+ 12`); filled `#212121`, outlined 1 point in the arrow's
/// colour (SVG's default `stroke-width`); its words 11.5 high in white (`font-size="11.5"`,
/// `fill="#fff"`), their baseline 4 below its middle (`ly + 4`).
pub const ARROW_LABEL_PAST: f32 = 22.0;
pub const ARROW_LABEL_SHIFT: f32 = 6.0;
pub const ARROW_LABEL_HEIGHT: f32 = 20.0;
pub const ARROW_LABEL_RADIUS: u8 = 4;
pub const ARROW_LABEL_PAD: f32 = 6.0;
pub const ARROW_LABEL_FILL: Color32 = GRAY_200;
pub const ARROW_LABEL_OUTLINE: f32 = 1.0;
pub const ARROW_LABEL_SIZE: f32 = 11.5;
pub const ARROW_LABEL_TEXT: Color32 = GRAY_1000;
pub const ARROW_LABEL_DROP: f32 = 4.0;
/// Where a press takes an arrow (`arrowHit`): nearer than 14 points to its shaft's end, or
/// than 6 to its shaft.
pub const ARROW_HIT_TIP: f32 = 14.0;
pub const ARROW_HIT_SHAFT: f32 = 6.0;
/// The flip arrow's shaft, from the profile along the sweep: half the distance arrow's.
/// Silico's own: the prototype draws no flip arrow (its Direction is the dialog's, `dlgRev`),
/// so it's drawn as the distance arrow is, shorter, with no dot and no label.
pub const FLIP_ARROW_LENGTH: f32 = 28.0;

/// The navigation bar's width and height, and its margin from the canvas's bottom (the
/// prototype's `.nav-bar`, 12 from the bottom). Its height is a 24 high button, 3 of padding,
/// and a 1 border, each above and below; its width, its seven buttons as app.js's
/// `renderCanvasUI` lays them out (five of 26, the display menu's 42, and the grid's 26),
/// its separator (7), the 1 between each, and the same padding and border at each side.
pub const NAVIGATION_BAR_WIDTH: f32 = 220.0;
pub const NAVIGATION_BAR_HEIGHT: f32 = 32.0;
pub const NAVIGATION_BAR_MARGIN: f32 = 12.0;

/// The navigation bar's frame (`.nav-bar`): Gray.200 inside a 1 Gray.250 border, `--r`'s
/// corners, 3 of padding inside the border, 1 between its items, and its shadow (`0 4px
/// 14px`, black at half).
pub const NAVIGATION_BAR_BG: Color32 = FLOATING_BG;
pub const NAVIGATION_BAR_BORDER: Color32 = PANEL_BORDER;
pub const NAVIGATION_BAR_PADDING: f32 = 3.0;
pub const NAVIGATION_BAR_GAP: f32 = 1.0;
pub const NAVIGATION_BAR_SHADOW: Shadow = Shadow {
    offset: [0, 4],
    blur: 14,
    spread: 0,
    color: with_alpha(GRAY_0, 128),
};

/// A navigation bar button's height and least width (`.nav-bar .ab-btn`: 24 and 26), and
/// its separator's height and its margin at each side (`.nav-bar .ab-sep`: 14 and 3).
pub const NAVIGATION_BAR_BUTTON_HEIGHT: f32 = 24.0;
pub const NAVIGATION_BAR_BUTTON_WIDTH: f32 = 26.0;
pub const NAVIGATION_BAR_SEPARATOR_HEIGHT: f32 = 14.0;
pub const NAVIGATION_BAR_SEPARATOR_MARGIN: f32 = 3.0;

/// How far above its button the display settings' menu opens (interact.js's
/// `displayMenu(r.left, r.top - 4, true)`: its bottom 4 above the button's top, its left at
/// the button's).
pub const NAVIGATION_BAR_MENU_GAP: f32 = 4.0;

/// A hint over the canvas (`.canvas-hint`, `crate::canvas_hint`): Gray.200 inside a 1
/// Gray.250 border, `--r`'s corners, the navigation bar's shadow (`0 4px 14px`, black at
/// half), padded 6 above and below, 8 at its right and 12 at its left, 10 between its icon,
/// its words, and its button; its words body text ([`TEXT_DEFAULT`], the page's colour),
/// their emphasis [`TEXT_STRONG`] (`.strong`), and its icon their colour (`currentColor`).
pub const CANVAS_HINT_BG: Color32 = FLOATING_BG;
pub const CANVAS_HINT_BORDER: Color32 = PANEL_BORDER;
pub const CANVAS_HINT_SHADOW: Shadow = NAVIGATION_BAR_SHADOW;
pub const CANVAS_HINT_PADDING: Margin = Margin {
    left: 12,
    right: 8,
    top: 6,
    bottom: 6,
};
pub const CANVAS_HINT_GAP: f32 = 10.0;
pub const CANVAS_HINT_TEXT: Color32 = TEXT_DEFAULT;
pub const CANVAS_HINT_STRONG: Color32 = TEXT_STRONG;
/// Where a hint stands, centred across the canvas: its top 18 below the canvas's
/// (`.canvas-hint`'s `top: 18px`), as Create Sketch's pick's does.
pub const CANVAS_HINT_TOP: f32 = 18.0;
/// Where the empty part's hint stands, centred across the canvas: its bottom 64 above the
/// canvas's (app.js's `top:auto;bottom:64px`, over `.canvas-hint`'s `top: 18px`), clear of
/// the navigation bar under it.
pub const CANVAS_HINT_BOTTOM: f32 = 64.0;

/// A small button's height (`.btn`): Copy MCP config, and the dialogs' buttons.
pub const BUTTON_HEIGHT: f32 = 22.0;

/// A ghost button's padding at its sides (`.btn-ghost`, as the start page pads it).
pub const GHOST_BUTTON_PADDING: f32 = 6.0;

/// A ghost button's fill, hovered (`.btn-ghost:hover`, Gray.300).
pub const GHOST_BUTTON_HOVER_BG: Color32 = WIDGET_BG;

// ---- The application bar (`#appbar`, `.ab-*`, `.doc-tab`) ----

/// The room macOS's window buttons take at the bar's left, where the title bar is the app
/// bar (`.traffic`: 12 of padding, three 12 point lights 8 apart, and 14 of padding).
pub const TRAFFIC_LIGHTS_WIDTH: f32 = 12.0 + 3.0 * 12.0 + 2.0 * 8.0 + 14.0;

/// The bar's padding at its right (`#appbar`), and at its left where no window buttons are.
pub const APP_BAR_PADDING: f32 = 8.0;

/// The space between the bar's items (`#appbar`'s `gap`).
pub const APP_BAR_ITEM_SPACING: f32 = 2.0;

/// New part's margin before it, after the documents' tabs (its `margin-left:4px`).
pub const APP_BAR_NEW_MARGIN: f32 = 4.0;

/// The bar's border below it (`#appbar`'s `border-bottom`, Gray.250).
pub const APP_BAR_BORDER: Color32 = PANEL_BORDER;

/// A bar button's height and least width (`.ab-btn`, `--btn`: `large_button_size`).
pub const APP_BAR_BUTTON_SIZE: f32 = 22.0;

/// A bar button's padding at its sides, and the gap between its icon and its text.
pub const APP_BAR_BUTTON_PADDING: f32 = 5.0;
pub const APP_BAR_BUTTON_GAP: f32 = 4.0;

/// A bar button hovered (`.ab-btn:hover`, Gray.300), and on, as Home is on the start page
/// (`.ab-btn.on`, Gray.250); each draws its icon and text [`TEXT_STRONG`]. At rest a bar
/// button has no fill and is [`TEXT_DEFAULT`]; disabled, [`TEXT_DISABLED`].
pub const APP_BAR_BUTTON_HOVER_BG: Color32 = WIDGET_BG;
pub const APP_BAR_BUTTON_ON_BG: Color32 = HOVER_BG;

/// The separator between the bar's buttons and the documents' tabs (`.ab-sep`): its height,
/// its colour, and its margin at each side.
pub const APP_BAR_SEPARATOR_HEIGHT: f32 = 16.0;
pub const APP_BAR_SEPARATOR: Color32 = PANEL_BORDER;
pub const APP_BAR_SEPARATOR_MARGIN: f32 = 6.0;

/// A document's tab (`.doc-tab`): its padding at its left and right, the gap between its
/// parts, and its widest, borders and all; a longer name ends in an ellipsis.
pub const DOC_TAB_PADDING_LEFT: f32 = 10.0;
pub const DOC_TAB_PADDING_RIGHT: f32 = 6.0;
pub const DOC_TAB_GAP: f32 = 6.0;
pub const DOC_TAB_MAX_WIDTH: f32 = 220.0;

/// A tab's 1 point borders, at its right and, the first's, at its left (Gray.250).
pub const DOC_TAB_BORDER: Color32 = PANEL_BORDER;

/// A tab's name at rest (Gray.550), hovered (`.doc-tab:hover`, Gray.800 on Gray.150), and
/// the shown document's (`.doc-tab.active`, white on Gray.200, its fill covering the bar's
/// border below it).
pub const DOC_TAB_TEXT: Color32 = TEXT_SUBDUED;
pub const DOC_TAB_HOVER_TEXT: Color32 = GRAY_800;
pub const DOC_TAB_HOVER_BG: Color32 = GRAY_150;
pub const DOC_TAB_ACTIVE_TEXT: Color32 = TEXT_STRONG;
pub const DOC_TAB_ACTIVE_BG: Color32 = GRAY_200;

/// The asterisk of a document with unsaved edits (`.doc-tab .dirty`, white).
pub const DOC_TAB_DIRTY: Color32 = TEXT_STRONG;

/// A tab's close button (`.doc-tab .x`), shown while the tab is hovered or shown: its side,
/// its corners, its icon (Gray.550), and its fill and icon hovered (Gray.350, white).
pub const DOC_TAB_CLOSE_SIZE: f32 = 16.0;
/// A tab's mark (`.doc-tab .mark`): a dot this wide, before its name, `DOC_TAB_GAP` from
/// it; red for errors, the changed-outside tone for a file changed on disk (`.outside`).
pub const DOC_TAB_MARK_SIZE: f32 = 6.0;
pub const DOC_TAB_MARK_ERRORS: Color32 = ERROR_TEXT;
pub const DOC_TAB_MARK_OUTSIDE: Color32 = CHANGED_OUTSIDE;
pub const DOC_TAB_CLOSE_CORNER_RADIUS: u8 = 3;
pub const DOC_TAB_CLOSE: Color32 = TEXT_SUBDUED;
pub const DOC_TAB_CLOSE_HOVER_BG: Color32 = GRAY_350;
pub const DOC_TAB_CLOSE_HOVER: Color32 = TEXT_STRONG;

/// The gap between a tooltip's title and its shortcut's key caps (`.tt-title`), and the
/// space above its description (`.tt-desc`, [`HEADER_SIZE`] text, [`TEXT_SUBDUED`]).
pub const TOOLTIP_TITLE_GAP: f32 = 8.0;
pub const TOOLTIP_DESCRIPTION_GAP: f32 = 2.0;

// ---- The toolbar (`#toolbar`, `.tb-*`, `.btn-finish`) ----

/// The toolbar's fill (`#toolbar`, Gray.100) and its border below it (Gray.250).
pub const TOOLBAR_BG: Color32 = PANEL_BG;
pub const TOOLBAR_BORDER: Color32 = PANEL_BORDER;

/// The border below the toolbar's tabs, and at each panel's right (`.tb-tabs`, `.tb-panel`,
/// Gray.200).
pub const TOOLBAR_DIVIDER: Color32 = GRAY_200;

/// The tabs' row (`.tb-tabs`): its padding at its left, and the gap between its items.
pub const TOOLBAR_TABS_PADDING: f32 = 8.0;
pub const TOOLBAR_TABS_GAP: f32 = 2.0;

/// The workspace's name, DESIGN (`.tb-ws`): its padding at its left, right, and bottom,
/// where its text sits; its margin after its border (Gray.250); its text (Gray.550).
pub const TOOLBAR_WORKSPACE_PADDING_LEFT: f32 = 4.0;
pub const TOOLBAR_WORKSPACE_PADDING_RIGHT: f32 = 10.0;
pub const TOOLBAR_WORKSPACE_PADDING_BOTTOM: f32 = 5.0;
pub const TOOLBAR_WORKSPACE_MARGIN: f32 = 6.0;
pub const TOOLBAR_WORKSPACE_BORDER: Color32 = PANEL_BORDER;
pub const TOOLBAR_WORKSPACE_TEXT: Color32 = TEXT_SUBDUED;

/// A tab (`.tb-tab`): its height, its padding at its sides, and its top corners; its text
/// (Gray.550), hovered (Gray.800); the shown tab's (`.active`, white on Gray.200); and the
/// contextual tab's, SKETCH (`.ctx`, Blue.900 on Blue.500).
pub const TOOLBAR_TAB_HEIGHT: f32 = 22.0;
pub const TOOLBAR_TAB_PADDING: f32 = 10.0;
pub const TOOLBAR_TAB_CORNER_RADIUS: u8 = SMALL_CORNER_RADIUS;
pub const TOOLBAR_TAB_TEXT: Color32 = TEXT_SUBDUED;
pub const TOOLBAR_TAB_HOVER_TEXT: Color32 = GRAY_800;
pub const TOOLBAR_TAB_ACTIVE_TEXT: Color32 = TEXT_STRONG;
pub const TOOLBAR_TAB_ACTIVE_BG: Color32 = GRAY_200;
pub const TOOLBAR_TAB_CONTEXT_TEXT: Color32 = ON_SELECTION;
pub const TOOLBAR_TAB_CONTEXT_BG: Color32 = SELECTION_BG;

/// The panels' row (`.tb-panels`): its padding at its sides; and a panel's (`.tb-panel`) at
/// its top and sides; the gap between its buttons (`.tb-row`).
pub const TOOLBAR_PANELS_PADDING: f32 = 6.0;
pub const TOOLBAR_PANEL_PADDING_TOP: f32 = 4.0;
pub const TOOLBAR_PANEL_PADDING_SIDE: f32 = 6.0;
pub const TOOLBAR_BUTTON_GAP: f32 = 1.0;

/// A toolbar button (`.tb-btn`): its corners; its icon at rest (Gray.750); hovered, white
/// on Gray.300; off, Gray.400.
pub const TOOLBAR_BUTTON_CORNER_RADIUS: u8 = SMALL_CORNER_RADIUS;
pub const TOOLBAR_BUTTON_ICON: Color32 = TEXT_DEFAULT;
pub const TOOLBAR_BUTTON_HOVER_BG: Color32 = WIDGET_BG;
pub const TOOLBAR_BUTTON_HOVER_ICON: Color32 = TEXT_STRONG;
pub const TOOLBAR_BUTTON_DISABLED_ICON: Color32 = TEXT_DISABLED;
/// A sketch tool on (`.tb-btn.on`: Blue.900 on Blue.500).
pub const TOOLBAR_BUTTON_ON_BG: Color32 = SELECTION_BG;
pub const TOOLBAR_BUTTON_ON_ICON: Color32 = ON_SELECTION;
/// A toggle button that's on (`.tb-btn.toggle-on`, SELECT's filters): white on Gray.250,
/// marked along its foot by a 2-point line in the selection's blue (`box-shadow: inset 0
/// -2px 0 var(--blue)`).
pub const TOOLBAR_BUTTON_TOGGLED_BG: Color32 = GRAY_250;
pub const TOOLBAR_BUTTON_TOGGLED_ICON: Color32 = GRAY_1000;
pub const TOOLBAR_BUTTON_TOGGLED_MARK: Color32 = SELECTION_BG;
pub const TOOLBAR_BUTTON_TOGGLED_MARK_WIDTH: f32 = 2.0;

/// A panel's label and its drop-down (`.tb-label`): its text's size and line; its padding
/// over and under, and at its sides; the gap before its arrow, and the arrow's side; its
/// corners; its margin above; its text (Gray.550), and hovered, white on Gray.250.
pub const TOOLBAR_LABEL_SIZE: f32 = 10.0;
pub const TOOLBAR_LABEL_LINE_HEIGHT: f32 = 14.0;
pub const TOOLBAR_LABEL_PADDING: Margin = Margin::symmetric(4, 1);
pub const TOOLBAR_LABEL_GAP: f32 = 2.0;
pub const TOOLBAR_LABEL_ARROW_SIZE: f32 = 8.0;
pub const TOOLBAR_LABEL_CORNER_RADIUS: u8 = 3;
pub const TOOLBAR_LABEL_MARGIN_TOP: f32 = 1.0;
pub const TOOLBAR_LABEL_TEXT: Color32 = TEXT_SUBDUED;
pub const TOOLBAR_LABEL_HOVER_TEXT: Color32 = TEXT_STRONG;
pub const TOOLBAR_LABEL_HOVER_BG: Color32 = HOVER_BG;

/// Finish Sketch at the panels' right end (`.tb-finish`, `.btn-finish`): the room at its
/// right; its height (a toolbar button's), its padding at its sides, and the gap after its
/// icon; its fill (Blue.500) and its text (white); its fill hovered (the prototype's
/// `#2a78e8`).
pub const TOOLBAR_FINISH_MARGIN: f32 = 8.0;
pub const TOOLBAR_FINISH_PADDING: f32 = 12.0;
pub const TOOLBAR_FINISH_GAP: f32 = 6.0;
pub const TOOLBAR_FINISH_BG: Color32 = SELECTION_BG;
pub const TOOLBAR_FINISH_TEXT: Color32 = TEXT_STRONG;
pub const TOOLBAR_FINISH_HOVER_BG: Color32 = hex(0x2a78e8);

// ---- The browser (`#browser`, `.panel-title`, `.row`, `.tree-empty`) ----

/// A side panel's title (`.panel-title`, the browser's and the property panel's): its
/// padding at the left (`--pad`) and at the right, and the gap between its parts (`padding:
/// 0 6px 0 var(--pad); gap: 6px`), in [`HEADER_SIZE`] capitals, [`TEXT_STRONG`], on a
/// [`TITLE_BAR_HEIGHT`] row; its buttons (`.panel-title .ab-btn`) 20 square.
pub const BROWSER_TITLE_PADDING: f32 = VIEW_PADDING as f32;
pub const PANEL_TITLE_PADDING_RIGHT: f32 = 6.0;
pub const PANEL_TITLE_GAP: f32 = 6.0;
pub const PANEL_TITLE_BUTTON_SIZE: f32 = 20.0;

/// The browser collapsed to its strip (`#main.browser-collapsed`, app.js's `renderBrowser`):
/// its one button, Show browser, an `.ab-btn` ([`APP_BAR_BUTTON_SIZE`] square) with `margin:
/// 2px`, so 2 in from the strip's top and left.
pub const BROWSER_STRIP_BUTTON_MARGIN: f32 = 2.0;

/// A row's padding at its left before its depth's indents (`6 + depth * 18`), at its right,
/// and the gap between its parts (`.row`).
pub const BROWSER_ROW_PADDING_LEFT: f32 = 6.0;
pub const BROWSER_ROW_PADDING_RIGHT: f32 = 6.0;
pub const BROWSER_ROW_GAP: f32 = 4.0;

/// A row's fill hovered (`.row:hover`, Gray.250), and selected (`.row.selected`, Blue.500),
/// where its name, its icon, and its meta are [`ON_SELECTION`].
pub const BROWSER_ROW_HOVER_BG: Color32 = HOVER_BG;
pub const BROWSER_ROW_SELECTED_BG: Color32 = SELECTION_BG;

/// A row's caret (`.row .caret`): its place's side, its chevron's, and its colour (Gray.700).
pub const BROWSER_CARET_SIDE: f32 = 14.0;
pub const BROWSER_CARET_ICON: f32 = 10.0;
pub const BROWSER_CARET: Color32 = GRAY_700;

/// A row's eye (`.row .eye`): its place's side and corners; its colour shown (Gray.550) and
/// hidden (`.eye.off`, Gray.400); and hovered (`.eye:hover`), Gray.1000 on Gray.350.
pub const BROWSER_EYE_SIDE: f32 = 18.0;
pub const BROWSER_EYE_CORNER_RADIUS: u8 = 3;
pub const BROWSER_EYE: Color32 = TEXT_SUBDUED;
pub const BROWSER_EYE_OFF: Color32 = TEXT_DISABLED;
pub const BROWSER_EYE_HOVER: Color32 = GRAY_1000;
pub const BROWSER_EYE_HOVER_BG: Color32 = GRAY_350;

/// What a hidden row's name and icon are drawn at (`.row.hidden-obj`'s `opacity: .45`).
pub const BROWSER_HIDDEN_OPACITY: f32 = 0.45;

/// A row's meta (`.r-meta`): [`HEADER_SIZE`], Gray.550.
pub const BROWSER_META: Color32 = TEXT_SUBDUED;

/// A sketch's prefix (`.row .prefix`): [`MONOSPACE_SIZE`], Gray.550; under defined
/// [`SKETCH_UNDER_DEFINED`], over defined [`SKETCH_OVER_DEFINED`].
pub const BROWSER_PREFIX: Color32 = TEXT_SUBDUED;

/// An empty group's words (`.tree-empty`): their padding at the left (`--pad` and 36), top,
/// and bottom, in [`HEADER_SIZE`] text, Gray.550.
pub const BROWSER_EMPTY_PADDING_LEFT: i8 = VIEW_PADDING + 36;
pub const BROWSER_EMPTY_PADDING_TOP: i8 = 2;
pub const BROWSER_EMPTY_PADDING_BOTTOM: i8 = 6;
pub const BROWSER_EMPTY: Color32 = TEXT_SUBDUED;

/// The space below the last row (`.panel-body`'s `padding-bottom`).
pub const BROWSER_PADDING_BOTTOM: f32 = 12.0;

// ---- The timeline (`#timeline`, `.tl-*`) ----

/// The pane: [`TIMELINE_HEIGHT`] high on [`BOTTOM_BAR_BG`], under a [`PANEL_BORDER`] line;
/// its padding at each side, and the gap between its label, its track, and its summary
/// (`#timeline`'s `padding: 0 8px; gap: 8px`).
pub const TIMELINE_PADDING: f32 = 8.0;
pub const TIMELINE_GAP: f32 = 8.0;

/// Its label (`.tl-label`): [`HEADER_SIZE`] capitals, Gray.550, in a column this wide.
pub const TIMELINE_LABEL_WIDTH: f32 = 68.0;
pub const TIMELINE_LABEL: Color32 = TEXT_SUBDUED;

/// A feature's item (`.tl-item`): its side, its corners, and the gap between two items
/// (`.tl-track`'s `gap`); its icon the prototype's large one (`svg.i.lg`).
pub const TIMELINE_ITEM_SIZE: f32 = TOOLBAR_BUTTON_SIZE;
pub const TIMELINE_ITEM_CORNER_RADIUS: u8 = SMALL_CORNER_RADIUS;
pub const TIMELINE_ITEM_GAP: f32 = 2.0;

/// An item's icon at rest (Gray.750), and hovered (`:hover`), Gray.1000 on Gray.300.
pub const TIMELINE_ITEM_ICON: Color32 = TEXT_DEFAULT;
pub const TIMELINE_ITEM_HOVER_ICON: Color32 = TEXT_STRONG;
pub const TIMELINE_ITEM_HOVER_BG: Color32 = WIDGET_BG;

/// A selected item (`.selected`): Blue.900 on Blue.500.
pub const TIMELINE_ITEM_SELECTED_ICON: Color32 = ON_SELECTION;
pub const TIMELINE_ITEM_SELECTED_BG: Color32 = SELECTION_BG;

/// A sketch's icon (`.sketch`), Gray.600, whether hovered or selected: styles.css gives it
/// after both.
pub const TIMELINE_SKETCH_ICON: Color32 = GRAY_600;

/// The ring of an item whose feature made what the canvas's hover is over
/// (`.hovered-from-canvas`): Gray.600 inside its border, round its corners (`box-shadow:
/// inset 0 0 0 1px var(--g600)`), this wide.
pub const TIMELINE_RING: Color32 = GRAY_600;
pub const TIMELINE_RING_WIDTH: f32 = 1.0;

/// A failed feature (`.err`): its icon Red.550 and a line of it along the item's bottom;
/// a warned one's (`.warn`) the warning's orange. The line's width.
pub const TIMELINE_ERROR: Color32 = ERROR_TEXT;
pub const TIMELINE_WARNING: Color32 = WARNING;
pub const TIMELINE_MARK_LINE_WIDTH: f32 = 2.0;

/// A suppressed item's opacity (`.suppressed`), and its strike: inset from the item's sides,
/// this wide, turned this far (`::after`'s `left: 5px; right: 5px; height: 1.5px; rotate(-35deg)`).
pub const TIMELINE_SUPPRESSED_OPACITY: f32 = 0.35;
pub const TIMELINE_STRIKE_INSET: f32 = 5.0;
pub const TIMELINE_STRIKE_WIDTH: f32 = 1.5;
pub const TIMELINE_STRIKE_DEGREES: f32 = -35.0;

/// A queued item's opacity (`.queued`), while a rebuild runs.
pub const TIMELINE_QUEUED_OPACITY: f32 = 0.45;

/// The running step's spinner (`.building::before`): a ring inset from the item, its top and
/// right halves drawn in Gray.1000, turning once in this many seconds.
pub const TIMELINE_SPINNER_INSET: f32 = 2.0;
pub const TIMELINE_SPINNER_WIDTH: f32 = 2.0;
pub const TIMELINE_SPINNER: Color32 = GRAY_1000;
pub const TIMELINE_SPINNER_TURN_SECONDS: f32 = 0.7;

/// A mark at an item's corner (`.badge`): its side, inset from the top right; filled with
/// the error's Red (`error_fg_color`) or the warning's orange, in a ring of the bar's grey.
pub const TIMELINE_BADGE_SIZE: f32 = 9.0;
pub const TIMELINE_BADGE_INSET: f32 = 1.0;
pub const TIMELINE_BADGE_RING_WIDTH: f32 = 1.5;
pub const TIMELINE_BADGE_RING: Color32 = BOTTOM_BAR_BG;
pub const TIMELINE_BADGE_ERROR: Color32 = ERROR_FILL;
pub const TIMELINE_BADGE_WARNING: Color32 = WARNING;
/// A feature another program changed (`.badge.outside`): a smaller dot, 7 at the same
/// inset, in [`CHANGED_OUTSIDE`], in the same ring.
pub const TIMELINE_BADGE_OUTSIDE_SIZE: f32 = 7.0;

/// Where a dragged feature would drop (`.drop-before::before`): a Blue.525 line this wide,
/// this far left of the item it would drop before, short of its top and bottom by this.
pub const TIMELINE_DROP: Color32 = FOCUS;
pub const TIMELINE_DROP_WIDTH: f32 = 2.0;
pub const TIMELINE_DROP_OFFSET: f32 = 2.0;
pub const TIMELINE_DROP_INSET: f32 = 4.0;

/// An empty timeline's words (`.tl-empty`): [`HEADER_SIZE`], Gray.550, this far in.
pub const TIMELINE_EMPTY: Color32 = TEXT_SUBDUED;
pub const TIMELINE_EMPTY_PADDING: f32 = 4.0;

/// The summary at the right (`.tl-right`): [`HEADER_SIZE`], Gray.550; its errors (`.err-text`)
/// Red.550 with the error's icon, `gap` apart (`.st-item`'s), each part padded this much
/// at its sides (`.st-item`'s `padding: 0 8px`).
pub const TIMELINE_SUMMARY: Color32 = TEXT_SUBDUED;
pub const TIMELINE_SUMMARY_ERRORS: Color32 = ERROR_TEXT;
pub const TIMELINE_SUMMARY_PADDING: f32 = 8.0;
/// The space between the summary's parts (`.tl-right`'s `gap: 2px`).
pub const TIMELINE_SUMMARY_SPACING: f32 = 2.0;
pub const TIMELINE_SUMMARY_GAP: f32 = 6.0;

// ---- Key caps (`.kbd`) ----

/// A key cap's fill (Gray.250).
pub const KEY_CAP_BG: Color32 = GRAY_250;

/// A key cap's outline (Gray.350).
pub const KEY_CAP_STROKE: Color32 = GRAY_350;

/// A key cap's text (Gray.800).
pub const KEY_CAP_TEXT: Color32 = GRAY_800;

/// A key cap's outline on a primary button: black at 25%.
pub const KEY_CAP_ON_PRIMARY_STROKE: Color32 = hex_alpha(0x000000, 64);

/// A key cap's text on a primary button (the prototype's `#444`).
pub const KEY_CAP_ON_PRIMARY_TEXT: Color32 = hex(0x444444);

/// A key cap's text size.
pub const KEY_CAP_TEXT_SIZE: f32 = 10.5;

/// A key cap's least width, its padding at each side of its text, and its border's width at
/// its top and sides, inside its width and height as `.kbd`'s `border-box` has them.
pub const KEY_CAP_MIN_WIDTH: f32 = 16.0;
pub const KEY_CAP_PADDING: f32 = 4.0;
pub const KEY_CAP_BORDER: f32 = 1.0;
/// A key cap's border at its bottom (`.kbd { border-bottom-width: 2px }`), in every key cap,
/// a button's (`.btn .kbd`) too.
pub const KEY_CAP_BORDER_BOTTOM: f32 = 2.0;
/// A key cap's line (`.kbd { line-height: 16px }`), from under its top border: its words sit
/// in its middle, a top border's width below the cap's.
pub const KEY_CAP_LINE_HEIGHT: f32 = 16.0;

/// A key cap's corners, and the space between two caps of one shortcut (`.keys`).
pub const KEY_CAP_CORNER_RADIUS: u8 = 3;
pub const KEY_CAP_GAP: f32 = 2.0;

// ---- The status bar's marks ----

/// A state's dot (`.state-dot`): its side.
pub const STATE_DOT_SIZE: f32 = 8.0;

/// The spinner beside work under way (`.spinner`): its side.
pub const SPINNER_SIZE: f32 = 11.0;

/// The spinner's ring (`.spinner`: `border: 1.5px solid` Gray.350), and its turning quarter
/// (`border-top-color` Gray.1000), once round every [`SPINNER_TURN_SECONDS`] (`spin .7s`).
pub const SPINNER_WIDTH: f32 = 1.5;
pub const SPINNER_TRACK: Color32 = GRAY_350;
pub const SPINNER_HEAD: Color32 = GRAY_1000;
pub const SPINNER_TURN_SECONDS: f32 = 0.7;

// ---- The status bar (`#status`, `.st-item`, `.st-sep`, `.st-btn`, `.progress`) ----

/// The bar's fill, its border above it, and its text (`#status`: Gray.150, a 1 px Gray.250
/// border on top, `--fs-small` in Gray.550).
pub const STATUS_BAR_BG: Color32 = BOTTOM_BAR_BG;
pub const STATUS_BAR_BORDER: Color32 = PANEL_BORDER;
pub const STATUS_TEXT: Color32 = TEXT_SUBDUED;

/// The bar's padding at its left and right, and the gap between its parts (`#status`:
/// `padding: 0 6px 0 8px; gap: 2px`).
pub const STATUS_BAR_PADDING_LEFT: f32 = 8.0;
pub const STATUS_BAR_PADDING_RIGHT: f32 = 6.0;
pub const STATUS_BAR_GAP: f32 = 2.0;

/// An item (`.st-item`): its height, its padding at each side, and the gap between its
/// mark and its words; hovered, as a button, its fill and its text.
pub const STATUS_ITEM_HEIGHT: f32 = 20.0;
pub const STATUS_ITEM_PADDING: f32 = 8.0;
pub const STATUS_ITEM_GAP: f32 = 6.0;
pub const STATUS_ITEM_HOVER_BG: Color32 = WIDGET_BG;
pub const STATUS_ITEM_HOVER_TEXT: Color32 = TEXT_STRONG;
/// In sketch mode, the pointer's coordinates' item's least width (`statusText`'s
/// `min-width: 150px`).
pub const STATUS_COORDINATES_WIDTH: f32 = 150.0;

/// A separator (`.st-sep`: 1 by 12, Gray.250), and the margin at each side of the one after
/// Stop and Rebuild (`margin: 0 6px`).
pub const STATUS_SEPARATOR: Color32 = PANEL_BORDER;
pub const STATUS_SEPARATOR_HEIGHT: f32 = 12.0;
pub const STATUS_SEPARATOR_MARGIN: f32 = 6.0;

/// A button (`.st-btn`): its height, padding at each side, and the gap after its icon; its
/// fill and text at rest, hovered, and off.
pub const STATUS_BUTTON_HEIGHT: f32 = 18.0;
pub const STATUS_BUTTON_PADDING: f32 = 7.0;
pub const STATUS_BUTTON_GAP: f32 = 5.0;
pub const STATUS_BUTTON_BG: Color32 = WIDGET_BG;
pub const STATUS_BUTTON_TEXT: Color32 = GRAY_800;
pub const STATUS_BUTTON_HOVER_BG: Color32 = WIDGET_HOVER_BG;
pub const STATUS_BUTTON_HOVER_TEXT: Color32 = GRAY_1000;
pub const STATUS_BUTTON_DISABLED_TEXT: Color32 = TEXT_DISABLED;

/// Stop, while it's on (`.st-btn.stop`): its fill and text, and hovered.
pub const STATUS_STOP_BG: Color32 = DANGER_BUTTON_BG;
pub const STATUS_STOP_TEXT: Color32 = DANGER_BUTTON_TEXT;
pub const STATUS_STOP_HOVER_BG: Color32 = hex(0x800027);
pub const STATUS_STOP_HOVER_TEXT: Color32 = GRAY_1000;

/// The rebuild's progress (`.progress`: 70 by 4, corners 2, Gray.300; its bar Gray.800).
pub const STATUS_PROGRESS_WIDTH: f32 = 70.0;
pub const STATUS_PROGRESS_HEIGHT: f32 = 4.0;
pub const STATUS_PROGRESS_CORNER_RADIUS: u8 = 2;
pub const STATUS_PROGRESS_BG: Color32 = WIDGET_BG;
pub const STATUS_PROGRESS_FILL: Color32 = GRAY_800;

// ---- The property panel (`.prop-head`, `.section`, `.prop`, `.hint-block`, `.err-card`) ----

/// The head (`.prop-head`): its padding above and below (`--pad` at its sides), the gap
/// between its icon and its words; its icon's box (28 square, `--r-sm`, Gray.200) and the
/// icon's colour (Gray.800); its title (13, white) and the line under it (`--fs-small`,
/// Gray.550).
pub const PROPERTY_HEAD_PADDING_TOP: f32 = 6.0;
pub const PROPERTY_HEAD_PADDING_BOTTOM: f32 = 8.0;
pub const PROPERTY_HEAD_GAP: f32 = 8.0;
pub const PROPERTY_HEAD_ICON_SIDE: f32 = 28.0;
pub const PROPERTY_HEAD_ICON_BG: Color32 = GRAY_200;
pub const PROPERTY_HEAD_ICON: Color32 = GRAY_800;
pub const PROPERTY_HEAD_TITLE: Color32 = TEXT_STRONG;
pub const PROPERTY_HEAD_SUB: Color32 = TEXT_SUBDUED;

/// A section's header (`.section`): its padding above and below, its text (10.5, capitals,
/// Gray.550), and the rule after it (1 high, Gray.200, 6 after the text).
pub const PROPERTY_SECTION_PADDING_TOP: f32 = 6.0;
pub const PROPERTY_SECTION_PADDING_BOTTOM: f32 = 2.0;
pub const PROPERTY_SECTION_SIZE: f32 = 10.5;
pub const PROPERTY_SECTION_TEXT: Color32 = TEXT_SUBDUED;
pub const PROPERTY_SECTION_GAP: f32 = 6.0;
pub const PROPERTY_SECTION_RULE: Color32 = GRAY_200;

/// A property's row (`.prop`): at least a list item high, its key's column 96 wide, 8 to
/// its value; its key Gray.550, its value Gray.800.
pub const PROPERTY_ROW_HEIGHT: f32 = LIST_ITEM_HEIGHT;
pub const PROPERTY_KEY_WIDTH: f32 = 96.0;
pub const PROPERTY_ROW_GAP: f32 = 8.0;
pub const PROPERTY_KEY: Color32 = TEXT_SUBDUED;
pub const PROPERTY_VALUE: Color32 = GRAY_800;
/// The gap between the parts of a row's value (`.prop .v { display: flex; gap: 6px }`): a
/// link and the words after it.
pub const PROPERTY_VALUE_GAP: f32 = 6.0;
/// The gap between a row's value and its copy button (`.prop .v`'s gap).
pub const PROPERTY_COPY_GAP: f32 = PROPERTY_VALUE_GAP;
/// A view's row of buttons under its rows (`selectionProps`' `padding: 10px var(--pad);
/// gap: 6px`): 10 above and below it, its buttons 6 apart.
pub const PROPERTY_BUTTONS_PADDING: f32 = 10.0;
pub const PROPERTY_BUTTONS_GAP: f32 = 6.0;

/// A hint (`.hint-block`): 8 above and below it, `--pad` at its sides; padded 8 by 10,
/// corners `--r`, Gray.150; its text `--fs-small` in Gray.550, its emphasis Gray.800.
pub const HINT_MARGIN: f32 = 8.0;
pub const HINT_PADDING: Margin = Margin::symmetric(10, 8);
pub const HINT_BG: Color32 = GRAY_150;
pub const HINT_TEXT: Color32 = TEXT_SUBDUED;
pub const HINT_STRONG: Color32 = GRAY_800;
/// A hint in a modal's body (`.hint-block` with `margin: 12px 0 0`): 12 above it, nothing
/// at its sides or under it.
pub const MODAL_HINT_MARGIN: Margin = Margin {
    left: 0,
    right: 0,
    top: 12,
    bottom: 0,
};

/// An error card (`.err-card`): 8 above and below it, `--pad` at its sides, corners `--r`;
/// its fill, its 1 px border, its head's text and icon (`.ec-head`), and its accent
/// (`.ec-fix::before`'s arrow); its words are white (`.ec-head .t`, `.ec-fix`).
pub const ERROR_CARD_MARGIN: f32 = 8.0;
pub const ERROR_CARD_BG: Color32 = ALERT_ERROR_BG;
pub const ERROR_CARD_BORDER: Color32 = DANGER_BUTTON_BG;
pub const ERROR_CARD_HEAD: Color32 = DANGER_BUTTON_TEXT;
pub const ERROR_CARD_ACCENT: Color32 = hex(0xff99a6);
/// A warning's card (`.err-card.warn`): its fill, border, and head.
pub const WARNING_CARD_BG: Color32 = ALERT_WARNING_BG;
pub const WARNING_CARD_BORDER: Color32 = hex(0x5c3a08);
pub const WARNING_CARD_HEAD: Color32 = ALERT_WARNING_TEXT;
/// The head's padding (`8px 10px 4px`) and the gap after its icon; the body's padding
/// (`0 10px 8px 32px`), and the space under its suggestion (`.ec-fix { margin-bottom: 8px;
/// gap: 6px }`).
pub const ERROR_CARD_HEAD_PADDING: Margin = Margin {
    left: 10,
    right: 10,
    top: 8,
    bottom: 4,
};
pub const ERROR_CARD_GAP: f32 = 8.0;
pub const ERROR_CARD_BODY_PADDING: Margin = Margin {
    left: 32,
    right: 10,
    top: 0,
    bottom: 8,
};
pub const ERROR_CARD_FIX_GAP: f32 = 6.0;
pub const ERROR_CARD_FIX_MARGIN: f32 = 8.0;
/// The body's words (`.ec-body`: an error's #ffe2e5, a warning's #ffe6c2), and the space
/// under each of its paragraphs, the core's message and its cause (`.ec-body p { margin: 0
/// 0 6px }`).
pub const ERROR_CARD_BODY_TEXT: Color32 = hex(0xffe2e5);
pub const WARNING_CARD_BODY_TEXT: Color32 = hex(0xffe6c2);
pub const ERROR_CARD_PARAGRAPH_MARGIN: f32 = 6.0;
/// The code chip at the head's right (`.ec-code`): 10 high in the monospace font, in the
/// accent, on black at 30% (77 of 255), padded 4 at its sides, corners 3.
pub const ERROR_CARD_CODE_SIZE: f32 = 10.0;
pub const ERROR_CARD_CODE_BG: Color32 = with_alpha(GRAY_0, 77);
pub const ERROR_CARD_CODE_PADDING: f32 = 4.0;
pub const ERROR_CARD_CODE_CORNER_RADIUS: u8 = 3;
/// The Feature, Elements, and Parameter rows (`.ec-kv`): `--fs-small`, its keys in the
/// accent, 8 from their values (its columns' gap) and 2 between rows, 6 under the last.
pub const ERROR_CARD_ROW_SIZE: f32 = HEADER_SIZE;
pub const ERROR_CARD_ROW_GAP: f32 = 8.0;
pub const ERROR_CARD_ROW_SPACING: f32 = 2.0;
pub const ERROR_CARD_ROWS_MARGIN: f32 = 6.0;
/// The feature's ID after its name (`.mono`, at `opacity: .7`).
pub const ERROR_CARD_ID_OPACITY: f32 = 0.7;
/// An element's chip (`.el-chip`): 10.5 high in the monospace font, white, on black at 35%
/// (89 of 255), padded 5 at its sides, corners 3; its name cut to 32 characters and an
/// ellipsis where it's longer than 34.
pub const ERROR_CARD_CHIP_SIZE: f32 = 10.5;
pub const ERROR_CARD_CHIP_BG: Color32 = with_alpha(GRAY_0, 89);
pub const ERROR_CARD_CHIP_TEXT: Color32 = GRAY_1000;
pub const ERROR_CARD_CHIP_PADDING: f32 = 5.0;
pub const ERROR_CARD_CHIP_CORNER_RADIUS: u8 = 3;
pub const ERROR_CARD_CHIP_LONGEST: usize = 34;
pub const ERROR_CARD_CHIP_KEPT: usize = 32;
/// The candidates (`errCard`'s heading, 10.5 in capitals in the accent, 4 above the first;
/// `.cand`: padded 5 by 6, corners `--r-sm`, black at 25% (64 of 255), 4 under each, its
/// name `.mono` in #ffd1d6).
pub const ERROR_CARD_CANDIDATES_SIZE: f32 = 10.5;
pub const ERROR_CARD_CANDIDATES_MARGIN: f32 = 4.0;
pub const ERROR_CARD_CANDIDATE_PADDING: Margin = Margin::symmetric(6, 5);
pub const ERROR_CARD_CANDIDATE_BG: Color32 = with_alpha(GRAY_0, 64);
pub const ERROR_CARD_CANDIDATE_MARGIN: f32 = 4.0;
pub const ERROR_CARD_CANDIDATE_TEXT: Color32 = hex(0xffd1d6);
/// A candidate's row while the pointer is over it, or over its edge in the canvas
/// (`.cand:hover`: white at 8%, 20 of 255).
pub const ERROR_CARD_CANDIDATE_HOVER_BG: Color32 = with_alpha(GRAY_1000, 20);
/// The gap between a candidate's name and its Use (`.cand`'s `gap: 8px`).
pub const ERROR_CARD_CANDIDATE_GAP: f32 = 8.0;
/// A candidate's why under its name (`.cand .why`: 10.5, in the accent).
pub const ERROR_CARD_CANDIDATE_WHY_SIZE: f32 = 10.5;
/// The card's buttons (`.ec-actions`: wrapped, 6 apart each way), each `.btn`
/// ([`BUTTON_HEIGHT`] high).
pub const ERROR_CARD_ACTIONS_GAP: f32 = 6.0;

// ---- The docked dialog (`.selbox`, `.chip`, `.seg`, `.dlg-footer`, `.btn-secondary`) ----

/// A selection box (`.selbox`): at least a field high, padded 6 at its left and 4 at its
/// right, 6 between its parts, a 2 px border at its left; its fill (Gray.200), and while
/// it's the active box its border Blue.500 and its fill Blue.500 at 18%.
pub const SELECTION_BOX_PADDING_LEFT: f32 = 6.0;
pub const SELECTION_BOX_PADDING_RIGHT: f32 = 4.0;
pub const SELECTION_BOX_BORDER: f32 = 2.0;
pub const SELECTION_BOX_BG: Color32 = FIELD_BG;
pub const SELECTION_BOX_ACTIVE_BORDER: Color32 = SELECTION_BG;
pub const SELECTION_BOX_ACTIVE_BG: Color32 = with_alpha(SELECTION_BG, 46);

/// What a selection box holds (`.selbox .chip`): 16 high, padded 5, 4 after its icon,
/// corners 3, Gray.300, its text `--fs-small` in Gray.800.
pub const CHIP_HEIGHT: f32 = 16.0;
pub const CHIP_PADDING: f32 = 5.0;
pub const CHIP_GAP: f32 = 4.0;
pub const CHIP_CORNER_RADIUS: u8 = 3;
pub const CHIP_BG: Color32 = WIDGET_BG;
pub const CHIP_TEXT: Color32 = GRAY_800;

/// A selection box's clear button (`renderDialog`'s Clear: an `.ab-btn` 18 high and wide),
/// beside each name a box of names lists.
pub const SELECTION_BOX_CLEAR_SIDE: f32 = 18.0;

/// What a box of names says under its names (`renderDialog`'s `.err-text` at 11 px): its
/// refusal, in `--red`.
pub const SELECTION_BOX_SAID: Color32 = ERROR_TEXT;

/// A box of names' note under its names (what of the selection it left out), nothing
/// wrong: the prototype's subdued text (`.subdued`, `--g550`).
pub const SELECTION_BOX_NOTE: Color32 = TEXT_SUBDUED;

/// A choice of a few (`.seg`): its fill (Gray.200), padding and gap 1; each choice 20 high,
/// corners 3, its text Gray.550 (white hovered), the one chosen Gray.350 with white text.
pub const SEGMENTED_BG: Color32 = FIELD_BG;
pub const SEGMENTED_PADDING: f32 = 1.0;
pub const SEGMENTED_HEIGHT: f32 = 20.0;
pub const SEGMENTED_CORNER_RADIUS: u8 = 3;
pub const SEGMENTED_TEXT: Color32 = TEXT_SUBDUED;
pub const SEGMENTED_HOVER_TEXT: Color32 = TEXT_STRONG;
pub const SEGMENTED_ON_BG: Color32 = GRAY_350;
pub const SEGMENTED_ON_TEXT: Color32 = TEXT_STRONG;

/// A dialog's choice of several, a drop-down (`.field.select`): a field's fill and corners,
/// padded 6 at each side, its words Gray.800, its arrow 10 square in Gray.550.
pub const SELECT_PADDING: f32 = 6.0;
pub const SELECT_TEXT: Color32 = GRAY_800;
pub const SELECT_ARROW: Color32 = TEXT_SUBDUED;
pub const SELECT_ARROW_SIZE: f32 = 10.0;

/// The preview's line in a dialog (`.dlg-status`): 8 above it, `--pad` at its sides; padded
/// 6 by 8, corners `--r-sm`, Gray.150; its words `--fs-small` Gray.550, Gray.750 while it
/// runs (`.running`), red with errors; 6 between its parts.
pub const DIALOG_STATUS_MARGIN: f32 = 8.0;
pub const DIALOG_STATUS_PADDING: Margin = Margin::symmetric(8, 6);
pub const DIALOG_STATUS_BG: Color32 = GRAY_150;
pub const DIALOG_STATUS_TEXT: Color32 = TEXT_SUBDUED;
pub const DIALOG_STATUS_RUNNING_TEXT: Color32 = TEXT_DEFAULT;
pub const DIALOG_STATUS_GAP: f32 = 6.0;

/// A field's unit inside it (`.field .unit`): Gray.550, 6 from the field's right.
pub const FIELD_UNIT: Color32 = TEXT_SUBDUED;
pub const FIELD_UNIT_PADDING: f32 = 6.0;

/// The dialog's footer (`.dlg-footer`): padded 8 by `--pad`, 6 between its buttons, a 1 px
/// Gray.250 border above it, on the panel's fill; each button 26 high, sharing its width.
pub const DIALOG_FOOTER_PADDING: f32 = 8.0;
pub const DIALOG_FOOTER_GAP: f32 = 6.0;
pub const DIALOG_FOOTER_BORDER: Color32 = PANEL_BORDER;
pub const DIALOG_BUTTON_HEIGHT: f32 = 26.0;

/// A secondary button (`.btn-secondary`): Cancel. Its fill and text, and hovered.
pub const SECONDARY_BUTTON_BG: Color32 = WIDGET_BG;
pub const SECONDARY_BUTTON_TEXT: Color32 = TEXT_DEFAULT;
pub const SECONDARY_BUTTON_HOVER_BG: Color32 = WIDGET_HOVER_BG;
pub const SECONDARY_BUTTON_HOVER_TEXT: Color32 = TEXT_STRONG;

/// The gap between a button's words and its key cap (`.btn { gap: 6px }`), the cap at 55%
/// (`.btn .kbd`: `opacity: .55`, its border and text the button's).
pub const BUTTON_GAP: f32 = 6.0;
pub const BUTTON_KEY_CAP_OPACITY: f32 = 0.55;
/// A button's key cap's margin at its left (`.btn .kbd { margin-left: 2px }`): the cap is
/// [`BUTTON_GAP`] and this after the button's words.
pub const BUTTON_KEY_CAP_MARGIN: f32 = 2.0;
/// A button's padding at its sides (`.btn { padding: 0 10px }`), and its height
/// (`.btn { height: 22px }`, [`BUTTON_HEIGHT`]) where nothing sets another.
pub const BUTTON_PADDING: f32 = 10.0;

// ---- Modals (`.backdrop`, `.modal`) ----

/// A modal's width, its border in it (`.modal { width: 440px }`, `border-box`), on
/// [`MODAL_BACKDROP`].
pub const MODAL_WIDTH: f32 = 440.0;
/// A modal's tallest: the window's height but this (`.modal { max-height: calc(100vh -
/// 80px) }`); its body scrolls past that (`.m-body { overflow: auto }`), its foot staying.
pub const MODAL_WINDOW_MARGIN: f32 = 80.0;
/// A modal's fill, its 1 px border, and its corners (`.modal`: Gray.200, Gray.250, `--r`).
pub const MODAL_BG: Color32 = FLOATING_BG;
pub const MODAL_BORDER: Color32 = PANEL_BORDER;
pub const MODAL_CORNER_RADIUS: u8 = NORMAL_CORNER_RADIUS;
/// A modal's shadow (`.modal`: `0 15px 50px rgba(0,0,0,.7)`).
pub const MODAL_SHADOW: Shadow = Shadow {
    offset: [0, 15],
    blur: 50,
    spread: 0,
    color: with_alpha(GRAY_0, 179),
};
/// A modal's head (`.m-head`): padded 12 above, 12 at its right, 8 under, 16 at its left;
/// its icon 8 from its title, which is 14 high in Gray.1000 on a line of the body's
/// ([`BODY_LINE_HEIGHT`]).
pub const MODAL_HEAD_PADDING: Margin = Margin {
    left: 16,
    right: 12,
    top: 12,
    bottom: 8,
};
pub const MODAL_HEAD_GAP: f32 = 8.0;
pub const MODAL_TITLE_SIZE: f32 = 14.0;
pub const MODAL_TITLE: Color32 = TEXT_STRONG;
/// A modal's body (`.m-body`): padded 16 at its sides and 12 under; its words Gray.750
/// (`p { color: var(--g750) }`), red where they say what went wrong (`.err-text`).
pub const MODAL_BODY_PADDING: Margin = Margin {
    left: 16,
    right: 16,
    top: 0,
    bottom: 12,
};
pub const MODAL_TEXT: Color32 = TEXT_DEFAULT;
/// A field's label in a modal (`.modal .lbl`): `--fs-small` in Gray.550, 10 above it and 4
/// under, on a line of the body's.
pub const MODAL_LABEL: Color32 = TEXT_SUBDUED;
pub const MODAL_LABEL_MARGIN_TOP: f32 = 10.0;
pub const MODAL_LABEL_MARGIN_BOTTOM: f32 = 4.0;
/// A field in a modal (`.modal .field { height: 26px }`), and a button beside one.
pub const MODAL_FIELD_HEIGHT: f32 = 26.0;
/// A modal's foot (`.m-foot`): padded 10 above, 16 at its sides, 14 under; its buttons at
/// its right, 6 apart, each 26 high and 76 wide at least (`.m-foot .btn`).
pub const MODAL_FOOT_PADDING: Margin = Margin {
    left: 16,
    right: 16,
    top: 10,
    bottom: 14,
};
pub const MODAL_FOOT_GAP: f32 = 6.0;
pub const MODAL_BUTTON_HEIGHT: f32 = DIALOG_BUTTON_HEIGHT;
pub const MODAL_BUTTON_MIN_WIDTH: f32 = 76.0;

// ---- What's Wrong (`renderModal`'s `whatswrong`) ----

/// What's Wrong's width (`.modal`, `style="width:560px"`), no wider than the window but 40
/// (`max-width: calc(100vw - 40px)`), and its body's padding (`.m-body`,
/// `style="padding:0 4px 8px"`), its cards each `--pad` in from that.
pub const WHATS_WRONG_WIDTH: f32 = 560.0;
pub const WHATS_WRONG_WINDOW_MARGIN: f32 = 40.0;
pub const WHATS_WRONG_BODY_PADDING: Margin = Margin {
    left: 4,
    right: 4,
    top: 0,
    bottom: 8,
};

// ---- The shortcut sheet (`.sheet`) ----

/// The sheet's width, its border in it (`.sheet { width: 920px }`), at most the window's
/// less this (`.modal { max-width: calc(100vw - 40px) }`).
pub const SHEET_WIDTH: f32 = 920.0;
pub const SHEET_WINDOW_MARGIN: f32 = 40.0;
/// Its filter (the head's `.field`, `width: 220px; height: 24px`), its icon at its left and
/// its text after it as a field's padding puts it (`.field input { padding: 0 6px }`).
pub const SHEET_FILTER_WIDTH: f32 = 220.0;
pub const SHEET_FILTER_HEIGHT: f32 = 24.0;
/// Its columns (`.cols { columns: 3 260px; column-gap: 28px }`): three, each 260 wide at
/// least, 28 apart.
pub const SHEET_COLUMNS: usize = 3;
pub const SHEET_COLUMN_MIN_WIDTH: f32 = 260.0;
pub const SHEET_COLUMN_GAP: f32 = 28.0;
/// A group (`.grp { margin-bottom: 14px }`), 14 above the next in its column.
pub const SHEET_GROUP_GAP: f32 = 14.0;
/// A group's title (`.grp-t`): 10.5 high capitals, spaced as the panels' ([`CAPITALS_LETTER_SPACING`]),
/// in [`TEXT_SUBDUED`] on a line of the body's, its note beside it at its right; then 4,
/// its 1 px [`PANEL_BORDER`] underline, and 4 more before its rows.
pub const SHEET_GROUP_TITLE_SIZE: f32 = 10.5;
pub const SHEET_GROUP_TITLE_TEXT: Color32 = TEXT_SUBDUED;
pub const SHEET_GROUP_TITLE_PADDING: f32 = 4.0;
pub const SHEET_GROUP_TITLE_BORDER: Color32 = PANEL_BORDER;
pub const SHEET_GROUP_TITLE_MARGIN: f32 = 4.0;
/// A command's row (`.sc-row`): 22 high, its name in [`TEXT_DEFAULT`] (`.d`), its caps at
/// its right, 3 apart (`.sheet .sc-row .keys { gap: 3px }`), 8 from its name at the least.
pub const SHEET_ROW_HEIGHT: f32 = 22.0;
pub const SHEET_ROW_TEXT: Color32 = TEXT_DEFAULT;
pub const SHEET_ROW_GAP: f32 = 8.0;
pub const SHEET_KEY_CAP_GAP: f32 = 3.0;
/// What the filter found in a name (`.sheet mark`): Blue.500 at 35% under white words.
pub const SHEET_MATCH_BG: Color32 = with_alpha(SELECTION_BG, 89);
pub const SHEET_MATCH_TEXT: Color32 = GRAY_1000;
/// The notes (`.sheet .note`): `--fs-small` ([`HEADER_SIZE`]) in [`TEXT_SUBDUED`], their
/// strong words (`.strong`) [`TEXT_STRONG`]. The note under the columns sits 6 under them,
/// over a 1 px [`PANEL_BORDER`] rule, 8 above its words.
pub const SHEET_NOTE_TEXT: Color32 = TEXT_SUBDUED;
pub const SHEET_NOTE_STRONG: Color32 = TEXT_STRONG;
pub const SHEET_NOTE_MARGIN: f32 = 6.0;
pub const SHEET_NOTE_PADDING: f32 = 8.0;
pub const SHEET_NOTE_BORDER: Color32 = PANEL_BORDER;

// ---- The command search (`.palette-modal`; DESIGN.md's *Command search*) ----

/// The box (`.palette-modal`): 520 wide, its border in it (`box-sizing: border-box`), 96 from
/// the window's top and centred across it; Gray.200 inside a 1 px Gray.350 border, `--r`'s
/// corners, and a modal's shadow (`0 15px 50px rgba(0,0,0,.7)`).
pub const SEARCH_WIDTH: f32 = 520.0;
pub const SEARCH_TOP: f32 = 96.0;
pub const SEARCH_BG: Color32 = FLOATING_BG;
pub const SEARCH_BORDER: Color32 = STROKE_STRONG;
pub const SEARCH_CORNER_RADIUS: u8 = NORMAL_CORNER_RADIUS;
pub const SEARCH_SHADOW: Shadow = Shadow {
    offset: [0, 15],
    blur: 50,
    spread: 0,
    color: with_alpha(GRAY_0, 179),
};
/// Its backdrop over the window (the prototype's inline `background: rgba(0,0,0,.35)`).
pub const SEARCH_BACKDROP: Color32 = with_alpha(GRAY_0, 89);
/// Its field's row (`.pm-input`): 38 high, its 1 px Gray.250 rule under it in that; padded
/// 12 at its sides, the search icon (Gray.550) 8 before the field, whose words are 13 high
/// in Gray.1000.
pub const SEARCH_INPUT_HEIGHT: f32 = 38.0;
pub const SEARCH_INPUT_PADDING: f32 = 12.0;
pub const SEARCH_INPUT_GAP: f32 = 8.0;
pub const SEARCH_INPUT_SIZE: f32 = 13.0;
pub const SEARCH_INPUT_TEXT: Color32 = TEXT_STRONG;
pub const SEARCH_ICON: Color32 = TEXT_SUBDUED;
pub const SEARCH_RULE: Color32 = PANEL_BORDER;
/// Its list (`.pm-list`): at most 340 high, its padding of 4 in that, scrolling past it; each
/// row a menu's item (`.menu .mi`) 28 high (`.palette-modal .mi`).
pub const SEARCH_LIST_MAX_HEIGHT: f32 = 340.0;
pub const SEARCH_LIST_PADDING: f32 = 4.0;
pub const SEARCH_ROW_HEIGHT: f32 = 28.0;
/// Its foot (`.pm-foot`): over a 1 px Gray.250 rule, padded 6 over and under and 12 at its
/// sides, its parts 14 apart, 10.5 high in Gray.550 on a line of the body's, its keys as key
/// caps.
pub const SEARCH_FOOT_PADDING: Margin = Margin {
    left: 12,
    right: 12,
    top: 6,
    bottom: 6,
};
pub const SEARCH_FOOT_GAP: f32 = 14.0;
pub const SEARCH_FOOT_SIZE: f32 = 10.5;
pub const SEARCH_FOOT_TEXT: Color32 = TEXT_SUBDUED;

// ---- Banners over the canvas (`.canvas-banner`) ----

/// A banner's place over the canvas: 10 from its left and top, 148 from its right, clear of
/// the ViewCube (`.canvas-banner { left: 10px; top: 10px }`, and `right: 148px` as
/// `renderBanner` sets it).
pub const BANNER_MARGIN: f32 = 10.0;
pub const BANNER_MARGIN_RIGHT: f32 = 148.0;
/// Where the prototype gives no rule (its canvas is never narrow), proposals: the least
/// width a banner keeps beside the ViewCube, below which it takes the canvas's width but
/// [`BANNER_MARGIN`] at each side; and the least room its words keep beside its buttons,
/// below which the buttons go under the words, [`BANNER_ACTION_GAP`] under them, in rows
/// as they fit, the words never narrower than this.
pub const BANNER_MIN_WIDTH: f32 = 320.0;
pub const BANNER_WORDS_MIN_WIDTH: f32 = 200.0;

// ---- The window's least size ----

/// The canvas's least size: room for a banner at its least width, [`BANNER_MIN_WIDTH`],
/// [`BANNER_MARGIN`] at each side (340); and for the ViewCube, [`VIEW_CUBE_MARGIN`] from
/// the top, over the navigation bar, [`NAVIGATION_BAR_MARGIN`] above and below it (164).
pub const CANVAS_MIN_WIDTH: f32 = BANNER_MIN_WIDTH + 2.0 * BANNER_MARGIN;
pub const CANVAS_MIN_HEIGHT: f32 = VIEW_CUBE_MARGIN
    + VIEW_CUBE_SIDE
    + NAVIGATION_BAR_MARGIN
    + NAVIGATION_BAR_HEIGHT
    + NAVIGATION_BAR_MARGIN;

/// The toolbar's least width: its widest row, sketch mode's (SKETCH's CREATE, MODIFY,
/// CONSTRAINTS, and INSPECT, from its left after `.tb-panels`' 6, then Finish Sketch in
/// `.tb-finish`'s 8 at each side and the row's 6 at its right), 881.5 in the app's fonts;
/// rounded up to the 8-point grid the prototype's columns are on (248, 296), 888. The test
/// `at_the_windows_least_width_the_app_bar_toolbar_and_status_bar_fit` lays it out there.
pub const TOOLBAR_MIN_WIDTH: f32 = 888.0;

/// The window's least size (Fusion's window has one; the prototype, a page, has none): the
/// wider of the toolbar's least ([`TOOLBAR_MIN_WIDTH`], 888) and the side panes' columns at
/// their leasts (`crate::pane_widths`' `BROWSER_MIN_W` and `PROPERTIES_MIN_W`, 200 and 232)
/// with the least canvas between them, 772: 888, at which the panes keep the prototype's
/// widths and a narrower window gives back only what panes dragged wider take; and the app
/// bar (`--top-bar-h`), the toolbar (`--toolbar-tabs-h`, `--toolbar-h`, and its border), the
/// least canvas, the timeline (`--timeline-h`, and the 1 egui's separator line takes above
/// it), and the status bar (`--status-h`), 336.
pub const WINDOW_MIN_WIDTH: f32 = TOOLBAR_MIN_WIDTH.max(
    crate::pane_widths::BROWSER_MIN_W + crate::pane_widths::PROPERTIES_MIN_W + CANVAS_MIN_WIDTH,
);
pub const WINDOW_MIN_HEIGHT: f32 = TOP_BAR_HEIGHT
    + TOOLBAR_TABS_HEIGHT
    + TOOLBAR_PANELS_HEIGHT
    + BORDER_WIDTH
    + CANVAS_MIN_HEIGHT
    + BORDER_WIDTH
    + TIMELINE_HEIGHT
    + STATUS_BAR_HEIGHT;
/// A banner's padding (`9px 10px 9px 12px`), inside its 1 px border, its corners `--r`; 10
/// between its icon, its words, and its buttons, which are 6 apart; its icon 1 lower than
/// its words' top (`.b-icon { margin-top: 1px }`); its second line 2 under its first, at
/// 85% (`.b-text .s`).
pub const BANNER_PADDING: Margin = Margin {
    left: 12,
    right: 10,
    top: 9,
    bottom: 9,
};
pub const BANNER_CORNER_RADIUS: u8 = NORMAL_CORNER_RADIUS;
pub const BANNER_GAP: f32 = 10.0;
pub const BANNER_ACTION_GAP: f32 = 6.0;
pub const BANNER_ICON_MARGIN_TOP: f32 = 1.0;
pub const BANNER_SUB_MARGIN_TOP: f32 = 2.0;
pub const BANNER_SUB_OPACITY: f32 = 0.85;
/// The banner's title (`.b-text .t`): Gray.1000.
pub const BANNER_TITLE: Color32 = TEXT_STRONG;
/// A banner with unsaved edits held over a file changed on disk (`.canvas-banner.warn`):
/// its fill, border, and words (`--alert-warn-text`).
pub const BANNER_WARNING_BG: Color32 = hex(0x2a1d0b);
pub const BANNER_WARNING_BORDER: Color32 = hex(0x6b4310);
pub const BANNER_WARNING_TEXT: Color32 = ALERT_WARNING_TEXT;
/// A banner saying the session reloaded its file (`.canvas-banner.info`): the info tone on
/// a dark card as the other banners' (`--info-card-bg`, `--info-card-border`,
/// `--info-card-text`; Trevin's change of 2026-11-09, from purple).
pub const BANNER_INFO_BG: Color32 = hex(0x111a2d);
pub const BANNER_INFO_BORDER: Color32 = hex(0x2b3c66);
pub const BANNER_INFO_TEXT: Color32 = hex(0xdce3ff);
/// A banner saying the file is gone (`.canvas-banner.err`).
pub const BANNER_ERROR_BG: Color32 = hex(0x2a0a12);
pub const BANNER_ERROR_BORDER: Color32 = hex(0x6e1228);
pub const BANNER_ERROR_TEXT: Color32 = DANGER_BUTTON_TEXT;
/// A button on a banner (`.canvas-banner .btn-secondary`): white at 8%, its words the
/// banner's; hovered, white at 16%, its words white.
pub const BANNER_BUTTON_BG: Color32 = with_alpha(GRAY_1000, 20);
pub const BANNER_BUTTON_HOVER_BG: Color32 = with_alpha(GRAY_1000, 41);
pub const BANNER_BUTTON_HOVER_TEXT: Color32 = GRAY_1000;
/// The list of what changed under a banner's words (`.canvas-banner .diff`): 6 under them,
/// in the small size, on rows 2 apart, its three columns 10 apart; the feature's name in
/// [`CHANGED_OUTSIDE`] (`.k`), the rest in the banner's words' colour.
pub const BANNER_LIST_MARGIN_TOP: f32 = 6.0;
pub const BANNER_LIST_ROW_GAP: f32 = 2.0;
pub const BANNER_LIST_COLUMN_GAP: f32 = 10.0;

/// The card in a feature's properties saying another program changed it (`.err-card.info`):
/// the info banner's card, its head ([`INFO`]) and words (Trevin's change of 2026-11-09,
/// from purple).
pub const OUTSIDE_CARD_BG: Color32 = BANNER_INFO_BG;
pub const OUTSIDE_CARD_BORDER: Color32 = BANNER_INFO_BORDER;
pub const OUTSIDE_CARD_HEAD: Color32 = INFO;
pub const OUTSIDE_CARD_TEXT: Color32 = BANNER_INFO_TEXT;
/// A ghost button's words (`.btn-ghost`): Gray.750; hovered, on its fill
/// ([`GHOST_BUTTON_HOVER_BG`]), white.
pub const GHOST_BUTTON_TEXT: Color32 = GRAY_750;
pub const GHOST_BUTTON_HOVER_TEXT: Color32 = GRAY_1000;

// ---- The start page (`.st-*`, `.recent-row`) ----

/// The page's widest, centred in the window.
pub const START_MAX_WIDTH: f32 = 1080.0;

/// The page's padding: above, at its sides, and below.
pub const START_PADDING_TOP: f32 = 44.0;
pub const START_PADDING_SIDE: f32 = 40.0;
pub const START_PADDING_BOTTOM: f32 = 40.0;

/// The left column's width (the name, New and Open, and the agents' note), and the gap to
/// the right column (Recent).
pub const START_SIDE_WIDTH: f32 = 240.0;
pub const START_COLUMN_GAP: f32 = 48.0;

/// The name's size, the logo's side beside it, the gap between them, and the space below.
pub const START_BRAND_SIZE: f32 = 18.0;
pub const START_LOGO_SIZE: f32 = 26.0;
pub const START_BRAND_GAP: f32 = 10.0;
pub const START_BRAND_MARGIN: f32 = 26.0;

/// An action's (New, Open) height, padding at its sides, the gap after its icon, and the
/// gap between two actions.
pub const START_ACTION_HEIGHT: f32 = 34.0;
pub const START_ACTION_PADDING: f32 = 10.0;
pub const START_ACTION_ICON_GAP: f32 = 10.0;
pub const START_ACTION_GAP: f32 = 4.0;

/// The primary action's fill, hovered (`.st-action.primary:hover`: white, where a primary
/// button's is Gray.850); its text is [`PRIMARY_BUTTON_HOVER_TEXT`].
pub const START_PRIMARY_HOVER_BG: Color32 = GRAY_1000;

/// The agents' note: the space above its rule, below it, its line height, and the space
/// above Copy MCP config.
pub const START_NOTE_MARGIN: f32 = 28.0;
pub const START_NOTE_PADDING: f32 = 16.0;
pub const START_NOTE_LINE_HEIGHT: f32 = 17.0;
pub const START_COPY_GAP: f32 = 10.0;

/// The space below a section header (RECENT).
pub const START_HEADER_GAP: f32 = 8.0;

/// A recent document's row: its height, its padding left and right, the gap between its
/// columns, the gap between two rows, and its fill hovered (Gray.200).
pub const START_RECENT_ROW_HEIGHT: f32 = 52.0;
pub const START_RECENT_PADDING_LEFT: f32 = 6.0;
pub const START_RECENT_PADDING_RIGHT: f32 = 10.0;
pub const START_RECENT_COLUMN_GAP: f32 = 14.0;
pub const START_RECENT_ROW_GAP: f32 = 1.0;
pub const START_RECENT_HOVER_BG: Color32 = GRAY_200;

/// The thumbnail's place in a recent document's row: its width and height, and its fill
/// (Gray.0).
pub const START_THUMB_WIDTH: f32 = 52.0;
pub const START_THUMB_HEIGHT: f32 = 40.0;
pub const START_THUMB_BG: Color32 = GRAY_0;

/// The note under Recent that a dropped document opens (`.drop-note`): the space above it
/// (`.recent`'s `margin-bottom: 30px`, the larger of the two margins that meet there while
/// Samples isn't built; the note's own is 26), its padding, and its border, 1 px dashed in
/// Gray.350, its dashes and gaps each 3 long, as a browser draws a 1 px dashed border.
pub const START_DROP_NOTE_MARGIN: f32 = 30.0;
pub const START_DROP_NOTE_PADDING: f32 = 14.0;
pub const START_DROP_NOTE_BORDER: Color32 = GRAY_350;
pub const START_DROP_NOTE_DASH: f32 = 3.0;
pub const START_DROP_NOTE_DASH_GAP: f32 = 3.0;

// ---- Type (DESIGN.md, *Type*) ----

/// Body text: 12 points (`re_ui`'s Body), on a 16 point line where a caller sets one.
pub const BODY_SIZE: f32 = 12.0;
/// Body text's line height, for a caller's `RichText::line_height` (egui's style has none).
pub const BODY_LINE_HEIGHT: f32 = 16.0;
/// Section headers, uppercase, and small text.
pub const HEADER_SIZE: f32 = 11.0;
/// The extra space between letters set in capitals (`text-transform: uppercase;
/// letter-spacing: .5px`): the start page's section headers, and the toolbar's workspace,
/// tabs, and panels' labels and drop-downs' headings.
pub const CAPITALS_LETTER_SPACING: f32 = 0.5;
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
    // egui's own darkest ground (a scroll bar's trough, a progress bar's, a dark frame's),
    // Gray.0 as the canvas was; the canvas is its gradient now (`VIEWPORT_BG_TOP`,
    // `VIEWPORT_BG_BOTTOM`).
    visuals.extreme_bg_color = GRAY_0;
    visuals.text_edit_bg_color = Some(FIELD_BG);
    visuals.code_bg_color = FIELD_BG;
    visuals.weak_text_color = Some(TEXT_SUBDUED);
    visuals.error_fg_color = ERROR_TEXT;
    visuals.warn_fg_color = WARNING;
    visuals.selection.bg_fill = SELECTION_BG;
    visuals.selection.stroke = Stroke::new(1.0, ON_SELECTION);
    visuals.window_corner_radius = CornerRadius::same(WINDOW_CORNER_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(NORMAL_CORNER_RADIUS);
    visuals.popup_shadow = POPUP_SHADOW;
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
        // A mouse press draws this look too, ring and all: egui gives pressed and focused
        // one look, so the ring for keyboard focus alone is the port's widgets' to draw.
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
    base.spacing.menu_margin = Margin::same(MENU_PADDING);
    base
}

/// Which of the prototype's text fields [`field_frame`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A dialog's field (`.field`): [`FIELD_BG`], unoutlined at rest, [`FIELD_HOVER_BG`]
    /// hovered. For a field on a panel ([`PANEL_BG`]): on a floating surface, whose fill is
    /// [`FIELD_BG`] too, it would show nothing at rest.
    Dialog,
    /// The Modify box's value (`.modify-box input`): [`MODIFY_FIELD_BG`], outlined in
    /// [`STROKE_STRONG`] at rest, hovered or not. The prototype's field on a floating
    /// surface ([`FLOATING_BG`], as the Modify box is), so a modal's fields too.
    Modify,
}

/// A text field's frame, as the prototype draws `kind`: its fill, and a 1 px outline,
/// [`FOCUS`]'s when `focused` and [`ERROR_TEXT`]'s when `invalid` (whether focused or not:
/// a value that doesn't parse shows while it's typed). A focused dialog field keeps its fill
/// at rest when hovered. The outline is there, transparent, on a dialog field at rest, so
/// the field keeps its size from state to state. For a `TextEdit` with its own frame off
/// (`.frame(false)`), whose text a caller colours [`TEXT_STRONG`], or [`ERROR_TEXT`] when
/// invalid.
pub fn field_frame(kind: FieldKind, hovered: bool, focused: bool, invalid: bool) -> egui::Frame {
    let (fill, rest) = match kind {
        FieldKind::Dialog if hovered && !focused => (FIELD_HOVER_BG, Color32::TRANSPARENT),
        FieldKind::Dialog => (FIELD_BG, Color32::TRANSPARENT),
        FieldKind::Modify => (MODIFY_FIELD_BG, STROKE_STRONG),
    };
    let outline = if invalid {
        ERROR_TEXT
    } else if focused {
        FOCUS
    } else {
        rest
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(FOCUS_WIDTH, outline))
        .corner_radius(CornerRadius::same(SMALL_CORNER_RADIUS))
        .inner_margin(Margin::symmetric(6, 2))
        .shadow(Shadow::NONE)
}

/// `edit` as the prototype draws a field of `kind`: in [`field_frame`]'s frame, hovered
/// while the pointer is over it (where it was last drawn) and it's on, focused while it has
/// the keyboard, and `invalid` as the caller says; its text [`TEXT_STRONG`], or
/// [`ERROR_TEXT`] when invalid. Its ID is made from `id_salt` in `ui`, as
/// `TextEdit::id_salt`'s is. egui's own frame for a text field can't be this: its fill is
/// one colour in every state, and its outline at rest is a button's, which has none.
pub fn field<'t>(
    ui: &egui::Ui,
    edit: egui::TextEdit<'t>,
    id_salt: impl egui::AsIdSalt,
    kind: FieldKind,
    invalid: bool,
) -> egui::TextEdit<'t> {
    field_with_room(ui, edit, id_salt, kind, invalid, 0.0)
}

/// [`field`], with `room` more points inside its frame at its right, past its text: where
/// its unit goes (`.field .unit`), which the caller paints there.
pub fn field_with_room<'t>(
    ui: &egui::Ui,
    edit: egui::TextEdit<'t>,
    id_salt: impl egui::AsIdSalt,
    kind: FieldKind,
    invalid: bool,
    room: f32,
) -> egui::TextEdit<'t> {
    field_with_rooms(ui, edit, id_salt, kind, invalid, [0.0, room])
}

/// [`field`], with `rooms[0]` more points inside its frame at its left, before its text
/// (where an icon goes, which the caller paints there), and `rooms[1]` at its right.
pub fn field_with_rooms<'t>(
    ui: &egui::Ui,
    edit: egui::TextEdit<'t>,
    id_salt: impl egui::AsIdSalt,
    kind: FieldKind,
    invalid: bool,
    rooms: [f32; 2],
) -> egui::TextEdit<'t> {
    let id = ui.make_persistent_id(id_salt);
    let hovered = ui
        .ctx()
        .read_response(id)
        .is_some_and(|drawn| drawn.enabled() && ui.rect_contains_pointer(drawn.rect));
    let focused = ui.memory(|memory| memory.has_focus(id));
    let mut frame = field_frame(kind, hovered, focused, invalid);
    let room = |room: f32| room.ceil().clamp(0.0, f32::from(i8::MAX)) as i8;
    frame.inner_margin.left = frame.inner_margin.left.saturating_add(room(rooms[0]));
    frame.inner_margin.right = frame.inner_margin.right.saturating_add(room(rooms[1]));
    edit.id(id)
        .frame(frame)
        .text_color(if invalid { ERROR_TEXT } else { TEXT_STRONG })
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, pos2, vec2};

    use super::*;
    use crate::modal::tests::{cover, painted};

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
        // Menus and popups (styles.css, `.menu`: 4 of padding, corners 6, and a shadow
        // `0 8px 28px` of black at 60%).
        assert_eq!(v.menu_corner_radius, CornerRadius::same(6));
        assert_eq!(
            v.popup_shadow,
            Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 0,
                color: Color32::from_rgba_unmultiplied(0, 0, 0, 153),
            }
        );
        assert_eq!(style.spacing.menu_margin, Margin::same(4));
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

    /// styles.css's `.field` (a dialog's) and `.modify-box input` (the Modify box's), each
    /// at rest, hovered, focused, and invalid.
    #[test]
    fn a_fields_frame_is_the_prototypes_in_each_state() {
        use FieldKind::{Dialog, Modify};
        let red = Color32::from_rgb(0xff, 0x38, 0x65);
        let blue = Color32::from_rgb(0x46, 0x76, 0xff);
        let none = Color32::TRANSPARENT;
        let g150 = Color32::from_rgb(0x17, 0x17, 0x17);
        let g200 = Color32::from_rgb(0x21, 0x21, 0x21);
        let g250 = Color32::from_rgb(0x27, 0x26, 0x26);
        let g350 = Color32::from_rgb(0x52, 0x51, 0x51);
        for (kind, hovered, focused, invalid, fill, outline) in [
            // `.field`: a transparent border at rest, Gray.250 hovered, its fill back and
            // the focus blue's border focused (`:focus-within` after `:hover`), and red
            // invalid (`.bad` after both).
            (Dialog, false, false, false, g200, none),
            (Dialog, true, false, false, g250, none),
            (Dialog, false, true, false, g200, blue),
            (Dialog, true, true, false, g200, blue),
            (Dialog, false, false, true, g200, red),
            (Dialog, true, false, true, g250, red),
            (Dialog, false, true, true, g200, red),
            (Dialog, true, true, true, g200, red),
            // `.modify-box input`: Gray.150 in a Gray.350 border, no hover of its own, the
            // focus blue's border focused, and red invalid (`.bad` after `:focus`).
            (Modify, false, false, false, g150, g350),
            (Modify, true, false, false, g150, g350),
            (Modify, false, true, false, g150, blue),
            (Modify, true, true, false, g150, blue),
            (Modify, false, false, true, g150, red),
            (Modify, true, false, true, g150, red),
            (Modify, false, true, true, g150, red),
            (Modify, true, true, true, g150, red),
        ] {
            let frame = field_frame(kind, hovered, focused, invalid);
            let state = format!("{kind:?} hovered {hovered} focused {focused} invalid {invalid}");
            assert_eq!(frame.fill, fill, "{state}");
            assert_eq!(frame.stroke, Stroke::new(1.0, outline), "{state}");
            assert_eq!(frame.corner_radius, CornerRadius::same(4), "{state}");
            // `padding: 0 6px`, in a 22 high field.
            assert_eq!(frame.inner_margin.left, 6, "{state}");
            assert_eq!(frame.inner_margin.right, 6, "{state}");
        }
    }

    /// The opacity in 255 steps, rounded to the nearest.
    fn of_255(fraction: f32) -> u8 {
        (fraction * 255.0f32).round() as u8
    }

    #[test]
    fn the_translucent_tokens_are_the_prototypes_colours_at_their_fractions_of_255() {
        // The prototype's colours and opacities, typed from styles.css and its scripts.
        // egui keeps a colour premultiplied, so each is compared in that form.
        for (name, color, [r, g, b], alpha) in [
            // `:focus-visible`'s `rgba(70,118,255,.25)`.
            ("FOCUS_HALO", FOCUS_HALO, [70, 118, 255], 0.25),
            // `--alert-red-bg`, `rgba(236, 0, 79, 0.18)`.
            ("ALERT_ERROR_BG", ALERT_ERROR_BG, [236, 0, 79], 0.18),
            // `--alert-warn-bg`, `rgba(223, 119, 0, 0.18)`.
            ("ALERT_WARNING_BG", ALERT_WARNING_BG, [223, 119, 0], 0.18),
            // sketch.js:291, `#4676ff` at `.35`.
            (
                "SKETCH_SELECTED_HALO",
                SKETCH_SELECTED_HALO,
                [0x46, 0x76, 0xff],
                0.35,
            ),
            // sketch.js:287, `#186add` at `.1`.
            (
                "SKETCH_PROFILE_FILL",
                SKETCH_PROFILE_FILL,
                [0x18, 0x6a, 0xdd],
                0.10,
            ),
            // sketch.js:273, `#000` at `.85`.
            (
                "SKETCH_DIMENSION_LABEL_BG",
                SKETCH_DIMENSION_LABEL_BG,
                [0, 0, 0],
                0.85,
            ),
            // `.backdrop`'s `rgba(0,0,0,.6)`.
            ("MODAL_BACKDROP", MODAL_BACKDROP, [0, 0, 0], 0.6),
            // `.menu`'s and `.tooltip`'s shadows, `rgba(0,0,0,.6)`.
            ("POPUP_SHADOW", POPUP_SHADOW.color, [0, 0, 0], 0.6),
            ("TOOLTIP_SHADOW", TOOLTIP_SHADOW.color, [0, 0, 0], 0.6),
            // `.modal`'s shadow, `rgba(0,0,0,.7)`.
            ("MODAL_SHADOW", MODAL_SHADOW.color, [0, 0, 0], 0.7),
            // `.canvas-hint`'s shadow, `rgba(0,0,0,.5)`.
            (
                "CANVAS_HINT_SHADOW",
                CANVAS_HINT_SHADOW.color,
                [0, 0, 0],
                0.5,
            ),
            // `.canvas-banner .btn-secondary`, `rgba(255,255,255,.08)`, `.16` hovered.
            ("BANNER_BUTTON_BG", BANNER_BUTTON_BG, [255, 255, 255], 0.08),
            (
                "BANNER_BUTTON_HOVER_BG",
                BANNER_BUTTON_HOVER_BG,
                [255, 255, 255],
                0.16,
            ),
        ] {
            assert_eq!(
                color,
                Color32::from_rgba_unmultiplied(r, g, b, of_255(alpha)),
                "{name}"
            );
        }
        // The renderer's, unmultiplied, so compared whole.
        for (name, color, [r, g, b], alpha) in [
            // viewport.js:587, `rgba(24,106,221,0.14)`.
            ("BOX_SELECT_FILL", BOX_SELECT_FILL, [24, 106, 221], 0.14),
            // viewport.js:183 and :186, `#3a3939` at 0.75 and `#646363` at 0.9.
            (
                "VIEWPORT_GRID_MINOR",
                VIEWPORT_GRID_MINOR,
                [0x3a, 0x39, 0x39],
                0.75,
            ),
            (
                "VIEWPORT_GRID_MAJOR",
                VIEWPORT_GRID_MAJOR,
                [0x64, 0x63, 0x63],
                0.9,
            ),
        ] {
            assert_eq!(color, [r, g, b, of_255(alpha)], "{name}");
        }
        // viewport.js's `buildGrid`, its `axis`: the grid's axes at 0.9.
        assert_eq!(VIEWPORT_GRID_AXIS_ALPHA, of_255(0.9));
    }

    /// The command search's tokens, typed from styles.css's `.palette-modal` rules and
    /// renderPalette's backdrop (`rgba(0,0,0,.35)`), not from the constants they're made of.
    #[test]
    fn the_command_searchs_tokens_are_the_prototypes() {
        assert_eq!(SEARCH_WIDTH, 520.0);
        assert_eq!(SEARCH_TOP, 96.0);
        assert_eq!(SEARCH_BG, Color32::from_rgb(0x21, 0x21, 0x21));
        assert_eq!(SEARCH_BORDER, Color32::from_rgb(0x52, 0x51, 0x51));
        assert_eq!(SEARCH_CORNER_RADIUS, 6);
        assert_eq!(
            SEARCH_SHADOW,
            Shadow {
                offset: [0, 15],
                blur: 50,
                spread: 0,
                color: Color32::from_rgba_unmultiplied(0, 0, 0, 179),
            }
        );
        assert_eq!(
            SEARCH_BACKDROP,
            Color32::from_rgba_unmultiplied(0, 0, 0, 89)
        );
        assert_eq!(
            (SEARCH_INPUT_HEIGHT, SEARCH_INPUT_PADDING, SEARCH_INPUT_GAP),
            (38.0, 12.0, 8.0)
        );
        assert_eq!(SEARCH_INPUT_SIZE, 13.0);
        assert_eq!(SEARCH_INPUT_TEXT, Color32::from_rgb(0xff, 0xff, 0xff));
        assert_eq!(SEARCH_ICON, Color32::from_rgb(0x93, 0x90, 0x90));
        assert_eq!(SEARCH_RULE, Color32::from_rgb(0x27, 0x26, 0x26));
        assert_eq!(
            (
                SEARCH_LIST_MAX_HEIGHT,
                SEARCH_LIST_PADDING,
                SEARCH_ROW_HEIGHT
            ),
            (340.0, 4.0, 28.0)
        );
        assert_eq!(
            SEARCH_FOOT_PADDING,
            Margin {
                left: 12,
                right: 12,
                top: 6,
                bottom: 6
            }
        );
        assert_eq!((SEARCH_FOOT_GAP, SEARCH_FOOT_SIZE), (14.0, 10.5));
        assert_eq!(SEARCH_FOOT_TEXT, Color32::from_rgb(0x93, 0x90, 0x90));
    }

    /// The prototype's colours for the tokens built from others, typed from its sources.
    #[test]
    fn the_tokens_named_for_others_are_the_prototypes_colours() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // sketch.js:7, `COL`, and styles.css's `--sk-*`.
        for (name, color, hex) in [
            ("SKETCH_UNDER_DEFINED", SKETCH_UNDER_DEFINED, 0x7a95ff),
            ("SKETCH_FULLY_DEFINED", SKETCH_FULLY_DEFINED, 0xeeeeed),
            ("SKETCH_OVER_DEFINED", SKETCH_OVER_DEFINED, 0xff3865),
            ("SKETCH_INVALID", SKETCH_INVALID, 0xf2c94c),
            ("SKETCH_DANGLING", SKETCH_DANGLING, 0xb5b84a),
            ("SKETCH_DRIVEN", SKETCH_DRIVEN, 0x827f7f),
            // sketch.js:308, :260, and :254.
            ("SKETCH_DIMENSION_TEXT", SKETCH_DIMENSION_TEXT, 0xcfcfcf),
            ("SKETCH_GLYPH_BG", SKETCH_GLYPH_BG, 0x212121),
            ("SKETCH_GLYPH_STROKE", SKETCH_GLYPH_STROKE, 0x525151),
            ("SKETCH_ORIGIN_RING", SKETCH_ORIGIN_RING.color, 0xcfcfcf),
            // sketch.js:341, :338, :346, and :347.
            (
                "SKETCH_PREVIEW_STROKE",
                SKETCH_PREVIEW_STROKE.color,
                0x7a95ff,
            ),
            ("SKETCH_GUIDE_STROKE", SKETCH_GUIDE_STROKE.color, 0x4676ff),
            ("SKETCH_SNAP_STROKE", SKETCH_SNAP_STROKE.color, 0xf2c94c),
            (
                "SKETCH_CROSSHAIR_STROKE",
                SKETCH_CROSSHAIR_STROKE.color,
                0xffffff,
            ),
            // styles.css: `.btn-primary:hover`, `:disabled`, `.btn-danger`, and its hover.
            (
                "PRIMARY_BUTTON_HOVER_TEXT",
                PRIMARY_BUTTON_HOVER_TEXT,
                0x000000,
            ),
            (
                "PRIMARY_BUTTON_DISABLED_BG",
                PRIMARY_BUTTON_DISABLED_BG,
                0x2c2b2b,
            ),
            (
                "PRIMARY_BUTTON_DISABLED_TEXT",
                PRIMARY_BUTTON_DISABLED_TEXT,
                0x827f7f,
            ),
            ("DANGER_BUTTON_BG", DANGER_BUTTON_BG, 0x5c001a),
            ("DANGER_BUTTON_TEXT", DANGER_BUTTON_TEXT, 0xffd1d6),
            ("DANGER_BUTTON_HOVER_BG", DANGER_BUTTON_HOVER_BG, 0x730022),
            (
                "DANGER_BUTTON_HOVER_TEXT",
                DANGER_BUTTON_HOVER_TEXT,
                0xffffff,
            ),
            // `.ab-btn:disabled`, `.row .r-icon`, `.row:hover`, `.field:hover`,
            // `.modify-box input`, and `--info`.
            ("TEXT_DISABLED", TEXT_DISABLED, 0x646363),
            ("ROW_ICON", ROW_ICON, 0x827f7f),
            ("TEXT_ROW_HOVER", TEXT_ROW_HOVER, 0xe0dfdf),
            ("FIELD_HOVER_BG", FIELD_HOVER_BG, 0x272626),
            ("MODIFY_FIELD_BG", MODIFY_FIELD_BG, 0x171717),
            ("INFO", INFO, 0xadbbff),
        ] {
            assert_eq!(color, opaque(hex), "{name}");
        }
        // viewport.js:587, viewport.js:183's `#646363`, and DESIGN.md's candidates and
        // canvas: styles.css's `--canvas-top` and `--canvas-bottom` (Trevin's of 2026-11-09),
        // and nothing behind the renderer's bodies (viewport.js's `setClearColor(0x000000,
        // 0)`).
        for (name, color, rgba) in [
            (
                "BOX_SELECT_STROKE",
                BOX_SELECT_STROKE,
                [0x46, 0x76, 0xff, 0xff],
            ),
            (
                "VIEWPORT_CANDIDATE",
                VIEWPORT_CANDIDATE,
                [0xff, 0x7a, 0x0c, 0xff],
            ),
            ("VIEWPORT_BG_TOP", VIEWPORT_BG_TOP, [0x14, 0x15, 0x17, 0xff]),
            (
                "VIEWPORT_BG_BOTTOM",
                VIEWPORT_BG_BOTTOM,
                [0x2a, 0x2c, 0x30, 0xff],
            ),
            // interact.js's `overlay`, its `dot`: a hovered vertex white, a selected one
            // #4676ff, each ringed `stroke="#000"`.
            (
                "VIEWPORT_HOVER_VERTEX",
                VIEWPORT_HOVER_VERTEX,
                [0xff, 0xff, 0xff, 0xff],
            ),
            (
                "VIEWPORT_SELECTION_VERTEX",
                VIEWPORT_SELECTION_VERTEX,
                [0x46, 0x76, 0xff, 0xff],
            ),
            (
                "VIEWPORT_VERTEX_RING_COLOR",
                VIEWPORT_VERTEX_RING_COLOR,
                [0, 0, 0, 0xff],
            ),
            // viewport.js's `MAKER`, `#d4dbff`: DESIGN.md's "a feature's faces on hover:
            // Blue.800", re_ui 0.38.1's Blue.800 (`color_table.ron`).
            (
                "VIEWPORT_FEATURE_HOVER_TINT",
                VIEWPORT_FEATURE_HOVER_TINT,
                [0xd4, 0xdb, 0xff, 0xff],
            ),
            // viewport.js's `buildPlanes` and `VP.setPlanes`: `#9f9f9f` at `opacity: 0.08`
            // filled (0.08 × 255 = 20.4), `lineMesh(…, '#9f9f9f', 1, 0.6)` outlined (153);
            // hovered `#186add` at 0.32 (81.6), its outline `#4676ff` at the outline's 0.6.
            (
                "VIEWPORT_PLANE_FILL",
                VIEWPORT_PLANE_FILL,
                [0x9f, 0x9f, 0x9f, 20],
            ),
            (
                "VIEWPORT_PLANE_OUTLINE",
                VIEWPORT_PLANE_OUTLINE,
                [0x9f, 0x9f, 0x9f, 153],
            ),
            (
                "VIEWPORT_PLANE_HOVER_FILL",
                VIEWPORT_PLANE_HOVER_FILL,
                [0x18, 0x6a, 0xdd, 82],
            ),
            (
                "VIEWPORT_PLANE_HOVER_OUTLINE",
                VIEWPORT_PLANE_HOVER_OUTLINE,
                [0x46, 0x76, 0xff, 153],
            ),
        ] {
            assert_eq!(color, rgba, "{name}");
        }
        assert_eq!(VIEWPORT_PLANE_WIDTH, 1.0);
        // viewport.js's `applyHighlight`: a maker's face `uTintAmt.value = 0.4` (DESIGN.md's
        // "tint 40%").
        assert_eq!(VIEWPORT_FEATURE_HOVER_TINT_AMOUNT, 0.4);
        assert_eq!(VIEWPORT_FRAME_BG, None);
        // The dot's `r="4.5"` and its ring's `stroke-width="1"`.
        assert_eq!((VIEWPORT_VERTEX_RADIUS, VIEWPORT_VERTEX_RING), (4.5, 1.0));
    }

    /// The app bar's and its tabs' values, typed from styles.css's `#appbar`, `.traffic`,
    /// `.ab-btn`, `.ab-sep`, `.doc-tab`, and `.tooltip`, and its `--g*` greys.
    #[test]
    fn the_app_bars_tokens_are_the_prototypes() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `.traffic`: `padding: 0 14px 0 12px`, three spans of 12, `gap: 8px`.
        assert_eq!(TRAFFIC_LIGHTS_WIDTH, 78.0);
        // `#appbar`: `padding-right: 8px; gap: 2px`, `--top-bar-h: 30px`.
        assert_eq!(APP_BAR_PADDING, 8.0);
        assert_eq!(APP_BAR_ITEM_SPACING, 2.0);
        assert_eq!(TOP_BAR_HEIGHT, 30.0);
        // `.ab-btn`: `height` and `min-width` `--btn` (22), `padding: 0 5px`, `gap: 4px`.
        assert_eq!(APP_BAR_BUTTON_SIZE, 22.0);
        assert_eq!(APP_BAR_BUTTON_PADDING, 5.0);
        assert_eq!(APP_BAR_BUTTON_GAP, 4.0);
        // `#appbar`'s `border-bottom`, `.ab-sep`'s `width`, and `.doc-tab`'s borders: 1px.
        assert_eq!(BORDER_WIDTH, 1.0);
        // `.ab-sep`: `height: 16px; margin: 0 6px`.
        assert_eq!(APP_BAR_SEPARATOR_HEIGHT, 16.0);
        assert_eq!(APP_BAR_SEPARATOR_MARGIN, 6.0);
        // `.doc-tab`: `padding: 0 6px 0 10px; gap: 6px; max-width: 220px`; `.x` 16 square
        // with corners 3.
        assert_eq!(DOC_TAB_PADDING_LEFT, 10.0);
        assert_eq!(DOC_TAB_PADDING_RIGHT, 6.0);
        assert_eq!(DOC_TAB_GAP, 6.0);
        assert_eq!(DOC_TAB_MAX_WIDTH, 220.0);
        assert_eq!(DOC_TAB_CLOSE_SIZE, 16.0);
        assert_eq!(DOC_TAB_CLOSE_CORNER_RADIUS, 3);
        assert_eq!(DOC_TAB_MARK_SIZE, 6.0);
        // `.tooltip .tt-title`'s `gap: 8px`, `.tt-desc`'s `margin-top: 2px`.
        assert_eq!(TOOLTIP_TITLE_GAP, 8.0);
        assert_eq!(TOOLTIP_DESCRIPTION_GAP, 2.0);
        for (name, color, hex) in [
            // `#appbar`'s `border-bottom` and `.ab-sep`, `--g250`.
            ("APP_BAR_BORDER", APP_BAR_BORDER, 0x272626),
            ("APP_BAR_SEPARATOR", APP_BAR_SEPARATOR, 0x272626),
            // `.ab-btn:hover`, `--g300`; `.ab-btn.on`, `--g250`.
            ("APP_BAR_BUTTON_HOVER_BG", APP_BAR_BUTTON_HOVER_BG, 0x2c2b2b),
            ("APP_BAR_BUTTON_ON_BG", APP_BAR_BUTTON_ON_BG, 0x272626),
            // `.doc-tab`, `--g550` in `--g250` borders; `:hover`, `--g800` on `--g150`;
            // `.active`, `--g1000` on `--g200`; `.dirty`, `--g1000`.
            ("DOC_TAB_TEXT", DOC_TAB_TEXT, 0x939090),
            ("DOC_TAB_BORDER", DOC_TAB_BORDER, 0x272626),
            ("DOC_TAB_HOVER_TEXT", DOC_TAB_HOVER_TEXT, 0xe0dfdf),
            ("DOC_TAB_HOVER_BG", DOC_TAB_HOVER_BG, 0x171717),
            ("DOC_TAB_ACTIVE_TEXT", DOC_TAB_ACTIVE_TEXT, 0xffffff),
            ("DOC_TAB_ACTIVE_BG", DOC_TAB_ACTIVE_BG, 0x212121),
            ("DOC_TAB_DIRTY", DOC_TAB_DIRTY, 0xffffff),
            // `.doc-tab .x`, `--g550`; `.x:hover`, `--g1000` on `--g350`.
            ("DOC_TAB_CLOSE", DOC_TAB_CLOSE, 0x939090),
            ("DOC_TAB_MARK_ERRORS", DOC_TAB_MARK_ERRORS, 0xff3865),
            ("DOC_TAB_MARK_OUTSIDE", DOC_TAB_MARK_OUTSIDE, 0xadbbff),
            ("DOC_TAB_CLOSE_HOVER", DOC_TAB_CLOSE_HOVER, 0xffffff),
            ("DOC_TAB_CLOSE_HOVER_BG", DOC_TAB_CLOSE_HOVER_BG, 0x525151),
        ] {
            assert_eq!(color, opaque(hex), "{name}");
        }
    }

    /// The toolbar's values, typed from styles.css's `#toolbar`, `.tb-*`, and
    /// `.btn-finish`, its `--toolbar-*` sizes, and its `--g*` greys and `--blue*`.
    #[test]
    fn the_toolbars_tokens_are_the_prototypes() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `--toolbar-tabs-h: 24px; --toolbar-h: 52px`.
        assert_eq!(TOOLBAR_TABS_HEIGHT, 24.0);
        assert_eq!(TOOLBAR_PANELS_HEIGHT, 52.0);
        // `.tb-tabs`: `padding-left: 8px; gap: 2px`.
        assert_eq!(TOOLBAR_TABS_PADDING, 8.0);
        assert_eq!(TOOLBAR_TABS_GAP, 2.0);
        // `.tb-ws`: `padding: 0 10px 5px 4px; margin-right: 6px`.
        assert_eq!(TOOLBAR_WORKSPACE_PADDING_LEFT, 4.0);
        assert_eq!(TOOLBAR_WORKSPACE_PADDING_RIGHT, 10.0);
        assert_eq!(TOOLBAR_WORKSPACE_PADDING_BOTTOM, 5.0);
        assert_eq!(TOOLBAR_WORKSPACE_MARGIN, 6.0);
        // `.tb-tab`: `padding: 0 10px; height: 22px`, `--r-sm` at its top; `letter-spacing:
        // .5px` there, on `.tb-ws`, `.tb-label`, `.menu .mh`, and `.st-h`.
        assert_eq!(TOOLBAR_TAB_HEIGHT, 22.0);
        assert_eq!(TOOLBAR_TAB_PADDING, 10.0);
        assert_eq!(TOOLBAR_TAB_CORNER_RADIUS, 4);
        assert_eq!(CAPITALS_LETTER_SPACING, 0.5);
        // `.tb-panels`: `padding: 0 6px`; `.tb-panel`: `padding: 4px 6px 0`; `.tb-row`:
        // `gap: 1px`.
        assert_eq!(TOOLBAR_PANELS_PADDING, 6.0);
        assert_eq!(TOOLBAR_PANEL_PADDING_TOP, 4.0);
        assert_eq!(TOOLBAR_PANEL_PADDING_SIDE, 6.0);
        assert_eq!(TOOLBAR_BUTTON_GAP, 1.0);
        // `.tb-btn`: 30 square, `--r-sm`.
        assert_eq!(TOOLBAR_BUTTON_SIZE, 30.0);
        assert_eq!(TOOLBAR_BUTTON_CORNER_RADIUS, 4);
        // `.tb-btn.toggle-on`: `box-shadow: inset 0 -2px 0`.
        assert_eq!(TOOLBAR_BUTTON_TOGGLED_MARK_WIDTH, 2.0);
        // `.tb-label`: `font-size: 10px; line-height: 14px; gap: 2px; padding: 1px 4px;
        // border-radius: 3px; margin-top: 1px`, its `svg` 8 square.
        assert_eq!(TOOLBAR_LABEL_SIZE, 10.0);
        assert_eq!(TOOLBAR_LABEL_LINE_HEIGHT, 14.0);
        assert_eq!(TOOLBAR_LABEL_GAP, 2.0);
        assert_eq!(TOOLBAR_LABEL_PADDING, Margin::symmetric(4, 1));
        assert_eq!(TOOLBAR_LABEL_CORNER_RADIUS, 3);
        assert_eq!(TOOLBAR_LABEL_MARGIN_TOP, 1.0);
        assert_eq!(TOOLBAR_LABEL_ARROW_SIZE, 8.0);
        // `.tb-finish`: `padding: 0 8px`; `.btn-finish`: `padding: 0 12px; gap: 6px`.
        assert_eq!(TOOLBAR_FINISH_MARGIN, 8.0);
        assert_eq!(TOOLBAR_FINISH_PADDING, 12.0);
        assert_eq!(TOOLBAR_FINISH_GAP, 6.0);
        for (name, color, hex) in [
            // `#toolbar`: `--g100`, its `border-bottom` `--g250`; `.tb-tabs`' and
            // `.tb-panel`'s borders, `--g200`; `.tb-ws`, `--g550` by a `--g250` border.
            ("TOOLBAR_BG", TOOLBAR_BG, 0x0d0d0d),
            ("TOOLBAR_BORDER", TOOLBAR_BORDER, 0x272626),
            ("TOOLBAR_DIVIDER", TOOLBAR_DIVIDER, 0x212121),
            ("TOOLBAR_WORKSPACE_TEXT", TOOLBAR_WORKSPACE_TEXT, 0x939090),
            (
                "TOOLBAR_WORKSPACE_BORDER",
                TOOLBAR_WORKSPACE_BORDER,
                0x272626,
            ),
            // `.tb-tab`, `--g550`; `:hover`, `--g800`; `.active`, `--g1000` on `--g200`;
            // `.ctx`, `--blue-text` on `--blue`.
            ("TOOLBAR_TAB_TEXT", TOOLBAR_TAB_TEXT, 0x939090),
            ("TOOLBAR_TAB_HOVER_TEXT", TOOLBAR_TAB_HOVER_TEXT, 0xe0dfdf),
            ("TOOLBAR_TAB_ACTIVE_TEXT", TOOLBAR_TAB_ACTIVE_TEXT, 0xffffff),
            ("TOOLBAR_TAB_ACTIVE_BG", TOOLBAR_TAB_ACTIVE_BG, 0x212121),
            (
                "TOOLBAR_TAB_CONTEXT_TEXT",
                TOOLBAR_TAB_CONTEXT_TEXT,
                0xf0f2ff,
            ),
            ("TOOLBAR_TAB_CONTEXT_BG", TOOLBAR_TAB_CONTEXT_BG, 0x186add),
            // `.tb-btn`, `--g750`; `:hover`, `--g1000` on `--g300`; `:disabled`, `--g400`.
            ("TOOLBAR_BUTTON_ICON", TOOLBAR_BUTTON_ICON, 0xcfcfcf),
            ("TOOLBAR_BUTTON_HOVER_BG", TOOLBAR_BUTTON_HOVER_BG, 0x2c2b2b),
            (
                "TOOLBAR_BUTTON_HOVER_ICON",
                TOOLBAR_BUTTON_HOVER_ICON,
                0xffffff,
            ),
            (
                "TOOLBAR_BUTTON_DISABLED_ICON",
                TOOLBAR_BUTTON_DISABLED_ICON,
                0x646363,
            ),
            // `.tb-btn.on`, `--blue-text` on `--blue`; `.toggle-on`, `--g1000` on `--g250`
            // over an inset line of `--blue`.
            ("TOOLBAR_BUTTON_ON_BG", TOOLBAR_BUTTON_ON_BG, 0x186add),
            ("TOOLBAR_BUTTON_ON_ICON", TOOLBAR_BUTTON_ON_ICON, 0xf0f2ff),
            (
                "TOOLBAR_BUTTON_TOGGLED_BG",
                TOOLBAR_BUTTON_TOGGLED_BG,
                0x272626,
            ),
            (
                "TOOLBAR_BUTTON_TOGGLED_ICON",
                TOOLBAR_BUTTON_TOGGLED_ICON,
                0xffffff,
            ),
            (
                "TOOLBAR_BUTTON_TOGGLED_MARK",
                TOOLBAR_BUTTON_TOGGLED_MARK,
                0x186add,
            ),
            // `.tb-label`, `--g550`; `:hover`, `--g1000` on `--g250`.
            ("TOOLBAR_LABEL_TEXT", TOOLBAR_LABEL_TEXT, 0x939090),
            (
                "TOOLBAR_LABEL_HOVER_TEXT",
                TOOLBAR_LABEL_HOVER_TEXT,
                0xffffff,
            ),
            ("TOOLBAR_LABEL_HOVER_BG", TOOLBAR_LABEL_HOVER_BG, 0x272626),
            // `.btn-finish`: `--g1000` on `--blue`; `:hover`, `#2a78e8`.
            ("TOOLBAR_FINISH_BG", TOOLBAR_FINISH_BG, 0x186add),
            ("TOOLBAR_FINISH_TEXT", TOOLBAR_FINISH_TEXT, 0xffffff),
            ("TOOLBAR_FINISH_HOVER_BG", TOOLBAR_FINISH_HOVER_BG, 0x2a78e8),
        ] {
            assert_eq!(color, opaque(hex), "{name}");
        }
    }

    /// The browser's values, typed from styles.css's `.panel-title`, `.row`, `.tree-empty`,
    /// and `.panel-body`, and its `--g*` greys, `--blue*`, and `--sk-*`.
    #[test]
    fn the_browsers_tokens_are_the_prototypes() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `.panel-title`: `padding: 0 6px 0 var(--pad)`, `--pad: 12px`.
        assert_eq!(BROWSER_TITLE_PADDING, 12.0);
        // The collapsed strip's button: `.ab-btn` (`--btn: 22px`) with `margin: 2px`.
        assert_eq!(BROWSER_STRIP_BUTTON_MARGIN, 2.0);
        assert_eq!(APP_BAR_BUTTON_SIZE, 22.0);
        // `.row`: `gap: 4px; padding-right: 6px`, `padding-left: 6 + depth * 18`.
        assert_eq!(BROWSER_ROW_PADDING_LEFT, 6.0);
        assert_eq!(BROWSER_ROW_PADDING_RIGHT, 6.0);
        assert_eq!(BROWSER_ROW_GAP, 4.0);
        assert_eq!(TREE_INDENT, 18.0);
        assert_eq!(LIST_ITEM_HEIGHT, 24.0);
        // `.row .caret`: 14 square, its `svg` 10; `.row .eye`: 18 square, `border-radius:
        // 3px`.
        assert_eq!(BROWSER_CARET_SIDE, 14.0);
        assert_eq!(BROWSER_CARET_ICON, 10.0);
        assert_eq!(BROWSER_EYE_SIDE, 18.0);
        assert_eq!(BROWSER_EYE_CORNER_RADIUS, 3);
        // `.row.hidden-obj .r-name, .row.hidden-obj .r-icon { opacity: .45 }`.
        assert_eq!(BROWSER_HIDDEN_OPACITY, 0.45);
        // `.tree-empty`: `padding: 2px 12px 6px calc(var(--pad) + 36px)`; `.panel-body`:
        // `padding-bottom: 12px`.
        assert_eq!(BROWSER_EMPTY_PADDING_LEFT, 48);
        assert_eq!(BROWSER_EMPTY_PADDING_TOP, 2);
        assert_eq!(BROWSER_EMPTY_PADDING_BOTTOM, 6);
        assert_eq!(BROWSER_PADDING_BOTTOM, 12.0);
        for (name, color, hex) in [
            // `.row`, `--g700`; `:hover`, `--g800` on `--g250`; `.selected`, `--blue-text`
            // on `--blue`.
            ("TEXT_ROW", TEXT_ROW, 0xbfbfbf),
            ("TEXT_ROW_HOVER", TEXT_ROW_HOVER, 0xe0dfdf),
            ("BROWSER_ROW_HOVER_BG", BROWSER_ROW_HOVER_BG, 0x272626),
            ("BROWSER_ROW_SELECTED_BG", BROWSER_ROW_SELECTED_BG, 0x186add),
            ("ON_SELECTION", ON_SELECTION, 0xf0f2ff),
            // `.row .caret`, `--g700`; `.r-icon`, `--g500`.
            ("BROWSER_CARET", BROWSER_CARET, 0xbfbfbf),
            ("ROW_ICON", ROW_ICON, 0x827f7f),
            // `.row .eye`, `--g550`; `.off`, `--g400`; `:hover`, `--g1000` on `--g350`.
            ("BROWSER_EYE", BROWSER_EYE, 0x939090),
            ("BROWSER_EYE_OFF", BROWSER_EYE_OFF, 0x646363),
            ("BROWSER_EYE_HOVER", BROWSER_EYE_HOVER, 0xffffff),
            ("BROWSER_EYE_HOVER_BG", BROWSER_EYE_HOVER_BG, 0x525151),
            // `.r-meta`, `.prefix`, and `.tree-empty`, `--g550`.
            ("BROWSER_META", BROWSER_META, 0x939090),
            ("BROWSER_PREFIX", BROWSER_PREFIX, 0x939090),
            ("BROWSER_EMPTY", BROWSER_EMPTY, 0x939090),
            // `.r-err` and `.row.err .r-name`, `--red`; `.r-warn`, `--orange`.
            ("ERROR_TEXT", ERROR_TEXT, 0xff3865),
            ("WARNING", WARNING, 0xff7a0c),
            // `.prefix.under`, `--sk-under` (`--blue-soft`); `.over`, `--sk-over` (`--red`).
            ("SKETCH_UNDER_DEFINED", SKETCH_UNDER_DEFINED, 0x7a95ff),
            ("SKETCH_OVER_DEFINED", SKETCH_OVER_DEFINED, 0xff3865),
        ] {
            assert_eq!(color, opaque(hex), "{name}");
        }
    }

    /// The timeline's values, typed from styles.css's `#timeline` and `.tl-*` rules, its
    /// `--timeline-h`, `--r-sm`, and its `--g*`, `--blue*`, `--red*`, and `--orange` colours.
    #[test]
    fn the_timelines_tokens_are_the_prototypes() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `#timeline`: `height: var(--timeline-h)` (40), `padding: 0 8px; gap: 8px`.
        assert_eq!(TIMELINE_HEIGHT, 40.0);
        assert_eq!(TIMELINE_PADDING, 8.0);
        assert_eq!(TIMELINE_GAP, 8.0);
        // `.tl-label`: `width: 68px`; `.tl-track`: `gap: 2px`; `.tl-item`: 30 square,
        // `border-radius: var(--r-sm)` (4).
        assert_eq!(TIMELINE_LABEL_WIDTH, 68.0);
        assert_eq!(TIMELINE_ITEM_GAP, 2.0);
        assert_eq!(TIMELINE_ITEM_SIZE, 30.0);
        assert_eq!(TIMELINE_ITEM_CORNER_RADIUS, 4);
        // `.hovered-from-canvas`: `inset 0 0 0 1px`.
        assert_eq!(TIMELINE_RING_WIDTH, 1.0);
        // `.err`/`.warn`: `inset 0 -2px 0`; `.suppressed`: `opacity: .35`, its strike
        // `left: 5px; right: 5px; height: 1.5px; rotate(-35deg)`; `.queued`: `.45`.
        assert_eq!(TIMELINE_MARK_LINE_WIDTH, 2.0);
        assert_eq!(TIMELINE_SUPPRESSED_OPACITY, 0.35);
        assert_eq!(TIMELINE_STRIKE_INSET, 5.0);
        assert_eq!(TIMELINE_STRIKE_WIDTH, 1.5);
        assert_eq!(TIMELINE_STRIKE_DEGREES, -35.0);
        assert_eq!(TIMELINE_QUEUED_OPACITY, 0.45);
        // `.building::before`: `inset: 2px; border: 2px`, `spin .7s`.
        assert_eq!(TIMELINE_SPINNER_INSET, 2.0);
        assert_eq!(TIMELINE_SPINNER_WIDTH, 2.0);
        assert_eq!(TIMELINE_SPINNER_TURN_SECONDS, 0.7);
        // `.badge`: `top: 1px; right: 1px; width: 9px`, ringed `0 0 0 1.5px var(--g150)`.
        assert_eq!(TIMELINE_BADGE_SIZE, 9.0);
        assert_eq!(TIMELINE_BADGE_INSET, 1.0);
        assert_eq!(TIMELINE_BADGE_RING_WIDTH, 1.5);
        // `.drop-before::before`: `left: -2px; top: 4px; bottom: 4px; width: 2px`.
        assert_eq!(TIMELINE_DROP_OFFSET, 2.0);
        assert_eq!(TIMELINE_DROP_INSET, 4.0);
        assert_eq!(TIMELINE_DROP_WIDTH, 2.0);
        // `.tl-empty`: `padding-left: 4px`; `.st-item`: `gap: 6px; padding: 0 8px`.
        assert_eq!(TIMELINE_EMPTY_PADDING, 4.0);
        assert_eq!(TIMELINE_SUMMARY_PADDING, 8.0);
        assert_eq!(TIMELINE_SUMMARY_GAP, 6.0);
        // `.tl-right`: `gap: 2px`.
        assert_eq!(TIMELINE_SUMMARY_SPACING, 2.0);
        for (name, color, hex) in [
            // `#timeline`, `--g150`; `.tl-label`, `.tl-empty`, `.tl-right`, `--g550`.
            ("BOTTOM_BAR_BG", BOTTOM_BAR_BG, 0x171717),
            ("TIMELINE_LABEL", TIMELINE_LABEL, 0x939090),
            ("TIMELINE_EMPTY", TIMELINE_EMPTY, 0x939090),
            ("TIMELINE_SUMMARY", TIMELINE_SUMMARY, 0x939090),
            // `.tl-item`, `--g750`; `:hover`, `--g1000` on `--g300`; `.selected`,
            // `--blue-text` on `--blue`; `.sketch`, `--g600`.
            ("TIMELINE_ITEM_ICON", TIMELINE_ITEM_ICON, 0xcfcfcf),
            (
                "TIMELINE_ITEM_HOVER_ICON",
                TIMELINE_ITEM_HOVER_ICON,
                0xffffff,
            ),
            ("TIMELINE_ITEM_HOVER_BG", TIMELINE_ITEM_HOVER_BG, 0x2c2b2b),
            (
                "TIMELINE_ITEM_SELECTED_ICON",
                TIMELINE_ITEM_SELECTED_ICON,
                0xf0f2ff,
            ),
            (
                "TIMELINE_ITEM_SELECTED_BG",
                TIMELINE_ITEM_SELECTED_BG,
                0x186add,
            ),
            ("TIMELINE_SKETCH_ICON", TIMELINE_SKETCH_ICON, 0x9f9f9f),
            // `.hovered-from-canvas`, `--g600`.
            ("TIMELINE_RING", TIMELINE_RING, 0x9f9f9f),
            // `.err`, `--red`; `.warn`, `--orange`; `.err-text`, `--red`.
            ("TIMELINE_ERROR", TIMELINE_ERROR, 0xff3865),
            ("TIMELINE_WARNING", TIMELINE_WARNING, 0xff7a0c),
            ("TIMELINE_SUMMARY_ERRORS", TIMELINE_SUMMARY_ERRORS, 0xff3865),
            // `.building::before`, `--g1000`.
            ("TIMELINE_SPINNER", TIMELINE_SPINNER, 0xffffff),
            // `.badge.err`, `--red-fill`; `.badge.warn`, `--orange`; their ring `--g150`.
            ("TIMELINE_BADGE_ERROR", TIMELINE_BADGE_ERROR, 0xab0116),
            ("TIMELINE_BADGE_WARNING", TIMELINE_BADGE_WARNING, 0xff7a0c),
            ("TIMELINE_BADGE_RING", TIMELINE_BADGE_RING, 0x171717),
            // `.drop-before::before`, `--blue-hi`.
            ("TIMELINE_DROP", TIMELINE_DROP, 0x4676ff),
        ] {
            assert_eq!(color, opaque(hex), "{name}");
        }
    }

    /// The prototype's sizes for the viewport's overlays and sketch mode.
    #[test]
    fn the_overlays_sizes_are_the_prototypes() {
        // viewport.js:101: `CUBE = 92, CUBE_M = 16`.
        assert_eq!(VIEW_CUBE_SIDE, 92.0);
        assert_eq!(VIEW_CUBE_MARGIN, 16.0);
        // DESIGN.md's *egui mapping*: "a 200 ms view transition".
        assert_eq!(VIEW_TRANSITION_SECONDS, 0.2);
        // `.nav-bar`: 12 from the bottom; `.nav-bar .ab-btn` 24 high, 3 of padding, and a
        // 1 border. Across: five buttons of `min-width: 26px`, the display menu's two 14
        // icons, 4 apart, in 5 of padding at each side, the grid's 26, the separator's 1
        // and 3 at each side, 1 between each of the eight, and the padding and border.
        assert_eq!(NAVIGATION_BAR_MARGIN, 12.0);
        assert_eq!(NAVIGATION_BAR_HEIGHT, 24.0 + 2.0 * 3.0 + 2.0 * 1.0);
        let display = 5.0 + 14.0 + 4.0 + 14.0 + 5.0;
        let contents = 5.0 * 26.0 + display + 26.0 + (1.0 + 2.0 * 3.0) + 7.0 * 1.0;
        assert_eq!(NAVIGATION_BAR_WIDTH, contents + 2.0 * 3.0 + 2.0 * 1.0);
        // `.palette`'s width, and `.sketch-bar`: a 22 high `.btn`, 4 of padding, a 1 border.
        assert_eq!(SKETCH_PALETTE_WIDTH, 212.0);
        assert_eq!(SKETCH_BAR_HEIGHT, 22.0 + 2.0 * 4.0 + 2.0 * 1.0);
        // sketch.js:254, :260, :265, :268, :273, :302, and :309.
        assert_eq!(SKETCH_ORIGIN_RING.width, 1.2);
        assert_eq!(SKETCH_ORIGIN_RING_RADIUS, 3.0);
        assert_eq!(SKETCH_GLYPH_SIZE, 16.0);
        assert_eq!(SKETCH_DIMENSION_GAP, 4.0);
        assert_eq!(SKETCH_DIMENSION_ARROW, [7.0, 6.0]);
        assert_eq!(SKETCH_DIMENSION_LABEL_PADDING, Margin::symmetric(5, 3));
        assert_eq!(SKETCH_GLYPH_OFFSET, 13.0);
        assert_eq!(SKETCH_DIMENSION_OFFSET, 26.0);
        // sketch.js:341, :338, :346, and :347.
        assert_eq!(SKETCH_PREVIEW_STROKE.width, 1.6);
        assert_eq!(SKETCH_CONSTRUCTION_DASH, [5.0, 3.0]);
        assert_eq!(SKETCH_GUIDE_STROKE.width, 1.0);
        assert_eq!(SKETCH_GUIDE_DASH, [2.0, 3.0]);
        assert_eq!(SKETCH_SNAP_STROKE.width, 1.5);
        assert_eq!(SKETCH_CROSSHAIR_STROKE.width, 1.0);
        // `.tooltip`: `padding: 6px 8px`, corners 4, shadow `0 6px 18px`.
        assert_eq!(TOOLTIP_PADDING, Margin::symmetric(8, 6));
        assert_eq!(TOOLTIP_CORNER_RADIUS, 4);
        assert_eq!(
            (
                TOOLTIP_SHADOW.offset,
                TOOLTIP_SHADOW.blur,
                TOOLTIP_SHADOW.spread
            ),
            ([0, 6], 18, 0)
        );
    }

    /// The ViewCube's look, typed from viewport.js's `label` and `buildCube`.
    #[test]
    fn the_view_cubes_look_is_the_prototypes() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `names`: RIGHT and LEFT `#2c2b2b`, TOP `#363535`, BOTTOM `#212121`, FRONT
        // `#303030`, BACK `#262626`; `strokeStyle = '#525151'`, `fillStyle = '#cfcfcf'`;
        // the edges' `color: '#827f7f'`.
        for (name, color, hex) in [
            ("VIEW_CUBE_SIDE_FILL", VIEW_CUBE_SIDE_FILL, 0x2c2b2b),
            ("VIEW_CUBE_TOP_FILL", VIEW_CUBE_TOP_FILL, 0x363535),
            ("VIEW_CUBE_BOTTOM_FILL", VIEW_CUBE_BOTTOM_FILL, 0x212121),
            ("VIEW_CUBE_FRONT_FILL", VIEW_CUBE_FRONT_FILL, 0x303030),
            ("VIEW_CUBE_BACK_FILL", VIEW_CUBE_BACK_FILL, 0x262626),
            ("VIEW_CUBE_BORDER", VIEW_CUBE_BORDER, 0x525151),
            ("VIEW_CUBE_LABEL", VIEW_CUBE_LABEL, 0xcfcfcf),
            ("VIEW_CUBE_EDGE", VIEW_CUBE_EDGE.color, 0x827f7f),
        ] {
            assert_eq!(color, opaque(hex), "{name}");
        }
        // A 128 px face: `lineWidth = 3`, `21px`, and the words' middle at 66, 2 below 64.
        assert_eq!(VIEW_CUBE_BORDER_WIDTH * 128.0, 3.0);
        assert_eq!(VIEW_CUBE_LABEL_SIZE * 128.0, 21.0);
        assert_eq!(VIEW_CUBE_LABEL_DROP * 128.0, 2.0);
        // A WebGL line's one pixel, the edges' and the axis stubs'.
        assert_eq!(VIEW_CUBE_EDGE.width, 1.0);
        assert_eq!(VIEW_CUBE_STUB_WIDTH, 1.0);
        // Home and ▾: `right: 16 + 92 + 4`, `gap: 2px`, `background: var(--g200)`; the menu
        // at `r.left - 150, r.bottom + 2` from the ▾, and `e.clientX - 160` from a click.
        assert_eq!(VIEW_CUBE_BUTTONS_GAP, 4.0);
        assert_eq!(VIEW_CUBE_BUTTONS_SPACING, 2.0);
        assert_eq!(VIEW_CUBE_BUTTON_BG, opaque(0x212121));
        assert_eq!(VIEW_CUBE_MENU_FROM_BUTTON, Vec2::new(-150.0, 2.0));
        assert_eq!(VIEW_CUBE_MENU_FROM_CLICK, Vec2::new(-160.0, 0.0));
    }

    /// The triad, typed from viewport.js's `drawOverlay`, and the view label, from
    /// styles.css's `.view-label` and the body's font, and app.js's `left:82px`.
    #[test]
    fn the_triads_and_the_view_labels_look_is_the_prototypes() {
        // `ox = 44, oy = h - 44, L = 26`; each `<line>` `stroke-width="2"`; each `<text>` at
        // `L + 9`, its `y` 3.5 below, `font-size="10.5"`; `<circle r="2" fill="#cfcfcf"/>`.
        assert_eq!(TRIAD_INSET, 44.0);
        assert_eq!(TRIAD_LENGTH, 26.0);
        assert_eq!(TRIAD_WIDTH, 2.0);
        assert_eq!(TRIAD_LABEL_PAST, 9.0);
        assert_eq!(TRIAD_LABEL_DROP, 3.5);
        assert_eq!(TRIAD_LABEL_SIZE, 10.5);
        assert_eq!(TRIAD_ORIGIN_RADIUS, 2.0);
        assert_eq!(TRIAD_ORIGIN_FILL, Color32::from_rgb(0xcf, 0xcf, 0xcf));
        // The axes' `#ff6981`, `#00b266`, `#7a95ff`, opaque.
        assert_eq!(AXIS_X, [0xff, 0x69, 0x81, 0xff]);
        assert_eq!(AXIS_Y, [0x00, 0xb2, 0x66, 0xff]);
        assert_eq!(AXIS_Z, [0x7a, 0x95, 0xff, 0xff]);
        // `left: 82px` (app.js), `bottom: 12px`, `--lh: 16px`, `--fs-small: 11px`,
        // `color: var(--g500)`, `#827f7f`.
        assert_eq!(VIEW_LABEL_LEFT, 82.0);
        assert_eq!(VIEW_LABEL_BOTTOM, 12.0);
        assert_eq!(VIEW_LABEL_LINE_HEIGHT, 16.0);
        assert_eq!(VIEW_LABEL_SIZE, 11.0);
        assert_eq!(VIEW_LABEL_TEXT, Color32::from_rgb(0x82, 0x7f, 0x7f));
    }

    /// The distance arrow, typed from interact.js's overlay of `arrowGeom`'s arrow, and its
    /// hit area from `arrowHit`.
    #[test]
    fn the_distance_arrows_look_is_the_prototypes() {
        // `col = dragArrow ? '#ffffff' : '#4676ff'`; `stroke-width="2.5"`; `ux * 56`.
        assert_eq!(ARROW, Color32::from_rgb(0x46, 0x76, 0xff));
        assert_eq!(ARROW_HELD, Color32::from_rgb(0xff, 0xff, 0xff));
        assert_eq!(ARROW_WIDTH, 2.5);
        assert_eq!(ARROW_LENGTH, 56.0);
        // The head's `g.ux * 12` and `px * 6`; the base's `r="3.5"`.
        assert_eq!(ARROW_HEAD_LENGTH, 12.0);
        assert_eq!(ARROW_HEAD_HALF_WIDTH, 6.0);
        assert_eq!(ARROW_DOT_RADIUS, 3.5);
        // `lx = tip.x + ux * 22 + 6`, `y = ly - 10`, `height="20"`, `rx="4"`,
        // `fill="#212121"`, `x = lx + 6`, `font-size="11.5"`, `fill="#fff"`, `y = ly + 4`.
        assert_eq!(ARROW_LABEL_PAST, 22.0);
        assert_eq!(ARROW_LABEL_SHIFT, 6.0);
        assert_eq!(ARROW_LABEL_HEIGHT, 20.0);
        assert_eq!(ARROW_LABEL_RADIUS, 4);
        assert_eq!(ARROW_LABEL_PAD, 6.0);
        assert_eq!(ARROW_LABEL_FILL, Color32::from_rgb(0x21, 0x21, 0x21));
        assert_eq!(ARROW_LABEL_OUTLINE, 1.0);
        assert_eq!(ARROW_LABEL_SIZE, 11.5);
        assert_eq!(ARROW_LABEL_TEXT, Color32::from_rgb(0xff, 0xff, 0xff));
        assert_eq!(ARROW_LABEL_DROP, 4.0);
        // `Math.hypot(m.x - g.tip.x, m.y - g.tip.y) < 14 || segD(m, g.a, g.tip) < 6`.
        assert_eq!(ARROW_HIT_TIP, 14.0);
        assert_eq!(ARROW_HIT_SHAFT, 6.0);
        assert_eq!(FLIP_ARROW_LENGTH, ARROW_LENGTH / 2.0);
    }

    /// The navigation bar, typed from styles.css's `.nav-bar`, `.nav-bar .ab-btn`, and
    /// `.nav-bar .ab-sep`, and app.js's buttons in it.
    #[test]
    fn the_navigation_bars_look_is_the_prototypes_and_its_size_its_parts() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `background: var(--g200); border: 1px solid var(--g250); padding: 3px; gap: 1px;
        // box-shadow: 0 4px 14px rgba(0,0,0,.5)`.
        assert_eq!(NAVIGATION_BAR_BG, opaque(0x212121));
        assert_eq!(NAVIGATION_BAR_BORDER, opaque(0x272626));
        assert_eq!(NAVIGATION_BAR_PADDING, 3.0);
        assert_eq!(NAVIGATION_BAR_GAP, 1.0);
        assert_eq!(
            (
                NAVIGATION_BAR_SHADOW.offset,
                NAVIGATION_BAR_SHADOW.blur,
                NAVIGATION_BAR_SHADOW.spread,
                NAVIGATION_BAR_SHADOW.color,
            ),
            ([0, 4], 14, 0, Color32::from_rgba_unmultiplied(0, 0, 0, 128))
        );
        // A button 24 high, 26 wide at least; a separator 14 high, 3 each side.
        assert_eq!(NAVIGATION_BAR_BUTTON_HEIGHT, 24.0);
        assert_eq!(NAVIGATION_BAR_BUTTON_WIDTH, 26.0);
        assert_eq!(NAVIGATION_BAR_SEPARATOR_HEIGHT, 14.0);
        assert_eq!(NAVIGATION_BAR_SEPARATOR_MARGIN, 3.0);
        // Seven buttons: five of one icon (14 and 5 each side, 24, so 26), Display settings'
        // two icons 4 apart (5 + 14 + 4 + 14 + 5 = 42), and Grid's 26; the separator's 1
        // and 3 each side; a 1 gap between each of the eight; the padding and border each
        // side: 5·26 + 42 + 26 + 7 + 7 + 2·4 = 220 across, 24 + 2·4 = 32 high.
        assert_eq!(NAVIGATION_BAR_WIDTH, 220.0);
        assert_eq!(NAVIGATION_BAR_HEIGHT, 32.0);
        assert_eq!(
            5.0 * NAVIGATION_BAR_BUTTON_WIDTH
                + 42.0
                + NAVIGATION_BAR_BUTTON_WIDTH
                + (BORDER_WIDTH + 2.0 * NAVIGATION_BAR_SEPARATOR_MARGIN)
                + 7.0 * NAVIGATION_BAR_GAP
                + 2.0 * (NAVIGATION_BAR_PADDING + BORDER_WIDTH),
            NAVIGATION_BAR_WIDTH
        );
        assert_eq!(
            NAVIGATION_BAR_BUTTON_HEIGHT + 2.0 * (NAVIGATION_BAR_PADDING + BORDER_WIDTH),
            NAVIGATION_BAR_HEIGHT
        );
        // The display menu 4 above its button (`r.top - 4`).
        assert_eq!(NAVIGATION_BAR_MENU_GAP, 4.0);
    }

    /// A hint over the canvas, typed from styles.css's `.canvas-hint` and app.js's empty
    /// part's `top:auto;bottom:64px`.
    #[test]
    fn the_canvas_hints_look_is_the_prototypes() {
        let opaque = |hex: u32| {
            let [_, r, g, b] = hex.to_be_bytes();
            Color32::from_rgb(r, g, b)
        };
        // `bottom: 64px`; `gap: 10px; padding: 6px 8px 6px 12px; background: var(--g200);
        // border: 1px solid var(--g250); border-radius: var(--r); box-shadow: 0 4px 14px
        // rgba(0,0,0,.5)`; the page's text `var(--g750)`, `.strong`'s `var(--g1000)`.
        assert_eq!(CANVAS_HINT_BOTTOM, 64.0);
        assert_eq!(CANVAS_HINT_TOP, 18.0);
        assert_eq!(CANVAS_HINT_GAP, 10.0);
        assert_eq!(
            CANVAS_HINT_PADDING,
            Margin {
                left: 12,
                right: 8,
                top: 6,
                bottom: 6
            }
        );
        assert_eq!(CANVAS_HINT_BG, opaque(0x212121));
        assert_eq!(CANVAS_HINT_BORDER, opaque(0x272626));
        assert_eq!(NORMAL_CORNER_RADIUS, 6);
        assert_eq!(
            (
                CANVAS_HINT_SHADOW.offset,
                CANVAS_HINT_SHADOW.blur,
                CANVAS_HINT_SHADOW.spread,
            ),
            ([0, 4], 14, 0)
        );
        assert_eq!(CANVAS_HINT_TEXT, opaque(0xcfcfcf));
        assert_eq!(CANVAS_HINT_STRONG, opaque(0xffffff));
    }

    // ---- A key cap's border, as `.kbd` draws it (border 1/1/2/1, radius 3, border-box) ----

    /// The feather egui gives an edge at two pixels a point, a Retina screen's: its one pixel
    /// (`feathering_size_in_pixels`) over the pixels per point, half a point.
    const RETINA_FEATHER: f32 = 0.5;

    /// A cover feathered over [`RETINA_FEATHER`] rises from nothing to whole linearly across
    /// it, so from a tenth to nine tenths over 0.8 of it.
    const RETINA_BAND: f32 = 0.8 * RETINA_FEATHER;

    /// Along the line from `from` to `to`, read in steps of 1/256 point and between them in a
    /// line: how far from `from` `mesh`'s cover first reaches each of `levels`.
    #[track_caller]
    fn reaches<const N: usize>(
        mesh: &egui::Mesh,
        from: Pos2,
        to: Pos2,
        levels: [f32; N],
    ) -> [f32; N] {
        const STEP: f32 = 1.0 / 256.0;
        let way = (to - from).normalized();
        let length = (to - from).length();
        levels.map(|level| {
            let mut last = cover(mesh, from);
            assert!(last < level, "{level} from {from:?}: {last} there already");
            let mut at = STEP;
            while at <= length {
                let now = cover(mesh, from + way * at);
                if now >= level {
                    return at - STEP + STEP * (level - last) / (now - last);
                }
                last = now;
                at += STEP;
            }
            panic!("the cover never reaches {level} from {from:?} to {to:?}")
        })
    }

    /// Where `mesh`'s cover is half, and how far it takes to rise from a tenth to nine tenths,
    /// from `from` towards `to`.
    #[track_caller]
    fn edge(mesh: &egui::Mesh, from: Pos2, to: Pos2) -> (f32, f32) {
        let [tenth, half, nine] = reaches(mesh, from, to, [0.1, 0.5, 0.9]);
        (half, nine - tenth)
    }

    /// Each corner of `rect`, clockwise from the top left, and the way into the box from it.
    fn corners(rect: Rect) -> [(Pos2, egui::Vec2); 4] {
        [
            (rect.left_top(), vec2(1.0, 1.0)),
            (rect.right_top(), vec2(-1.0, 1.0)),
            (rect.right_bottom(), vec2(-1.0, -1.0)),
            (rect.left_bottom(), vec2(1.0, -1.0)),
        ]
    }

    /// A border's mesh has no point that isn't a number.
    #[track_caller]
    fn finite(mesh: &egui::Mesh, what: &str) {
        let bad: Vec<Pos2> = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.pos)
            .filter(|pos| !pos.x.is_finite() || !pos.y.is_finite())
            .collect();
        assert!(bad.is_empty(), "{what}'s border has points {bad:?}");
    }

    /// At two pixels a point, egui feathers an edge over one pixel, so `key_cap_box` fades a
    /// key cap's border in over half a point ([`RETINA_FEATHER`]), centred on each of its
    /// edges: half covered on the edge, and from a tenth to nine tenths over 0.4 points
    /// ([`RETINA_BAND`]). Along the lines through the middle of a box at (20, 20) 24 by 16, on
    /// the half-point grid as it is: its outer edges on the box, 1 in from each line's start
    /// 1 outside it; its inner edges 1 in at its top and sides and 2 at its bottom, 7 up from
    /// its middle, 11 to each side, and 6 down. A feather of a pixel a point, or of the
    /// pixel's size times the pixels a point, is twice as wide.
    #[test]
    fn a_key_caps_border_is_feathered_over_a_pixel_at_two_pixels_a_point() {
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(24.0, 16.0));
        let mut h = egui_kittest::Harness::builder()
            .with_pixels_per_point(2.0)
            .with_size(vec2(80.0, 60.0))
            .build_ui(move |ui| key_cap_box(ui, rect, Color32::TRANSPARENT, Color32::WHITE));
        h.run();
        let meshes: Vec<egui::Mesh> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Mesh(mesh) => Some((*mesh).clone()),
                _ => None,
            })
            .collect();
        assert_eq!(meshes.len(), 1, "the key cap's border");
        let mesh = &meshes[0];
        let middle = pos2(32.0, 28.0);
        let lines = [
            (pos2(32.0, 19.0), middle, 1.0),
            (pos2(45.0, 28.0), middle, 1.0),
            (pos2(32.0, 37.0), middle, 1.0),
            (pos2(19.0, 28.0), middle, 1.0),
            (middle, pos2(32.0, 19.0), 7.0),
            (middle, pos2(45.0, 28.0), 11.0),
            (middle, pos2(32.0, 37.0), 6.0),
            (middle, pos2(19.0, 28.0), 11.0),
        ];
        for (from, to, want) in lines {
            let (half, band) = edge(mesh, from, to);
            assert!(
                (half - want).abs() <= 1.0 / 512.0 && (band - RETINA_BAND).abs() <= 1.0 / 512.0,
                "from {from:?} towards {to:?}: half covered {half} in and feathered over \
                 {band}, not {want} and {RETINA_BAND}"
            );
        }
    }

    /// A cap narrower or shorter than its corners' two radii: CSS scales `border-radius`
    /// down until the radii at each side fit it, so a cap 4 across or 4 high has corners of
    /// 2. Each corner's outer edge is then a circle of 2 about the point 2 in on each axis,
    /// crossing its diagonal 2 − 2/√2 = 0.586 in on each axis (not a radius of 3's 0.879).
    /// Its bottom corners' inner radii are 2 − 1 across and 2 − 2, none, down: a square inner
    /// corner, so the edge above the bottom border is straight, 2 up, half covered there and
    /// feathered over [`RETINA_BAND`] down the cap's middle, and no point of the border is
    /// other than a number.
    #[test]
    fn a_key_caps_corners_fit_a_cap_narrower_or_shorter_than_them() {
        let caps = [
            Rect::from_min_size(pos2(20.0, 20.0), vec2(4.0, 16.0)),
            Rect::from_min_size(pos2(20.0, 20.0), vec2(16.0, 4.0)),
        ];
        let want = 2.0 - std::f32::consts::SQRT_2;
        for rect in caps {
            let mesh = key_cap_border(rect, Color32::WHITE, RETINA_FEATHER);
            finite(&mesh, &format!("{rect:?}"));
            for (corner, inward) in corners(rect) {
                let [half] = reaches(&mesh, corner, corner + inward * 1.5, [0.5]);
                let got = half * std::f32::consts::FRAC_1_SQRT_2;
                assert!(
                    (got - want).abs() <= 1.0 / 512.0,
                    "{rect:?}'s corner at {corner:?}: its outer edge {got} in on its \
                     diagonal, not {want}"
                );
            }
            let foot = pos2(rect.center().x, rect.bottom() - 2.0);
            let from = foot - vec2(0.0, 0.5);
            let (half, band) = edge(&mesh, from, foot + vec2(0.0, 0.5));
            assert!(
                (half - 0.5).abs() <= 1.0 / 512.0 && (band - RETINA_BAND).abs() <= 1.0 / 512.0,
                "{rect:?}'s bottom inner edge: half covered {half} below {from:?} and \
                 feathered over {band}, not 0.5 and {RETINA_BAND}"
            );
        }
    }

    /// Each outer corner is a quarter circle of 3 about the point 3 in on each axis: it
    /// leaves its side 3 from the corner along it (its 0°), meets the next side 3 from the
    /// corner along that (its 90°), and crosses its diagonal 3 − 3/√2 = 0.879 in on each
    /// axis. The quarter is drawn in eight steps, so a point at each of those three: each is
    /// half covered exactly there, in the line across the edge through it, not only within
    /// the steps' chords (a quarter turned by a few degrees moves each by about a hundredth).
    #[test]
    fn a_key_caps_outer_corners_are_quarter_circles_where_css_puts_them() {
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(24.0, 16.0));
        let mesh = key_cap_border(rect, Color32::WHITE, RETINA_FEATHER);
        let diagonal = 3.0 - 3.0 * std::f32::consts::FRAC_1_SQRT_2;
        for (corner, inward) in corners(rect) {
            let along_x = corner + vec2(inward.x * 3.0, 0.0);
            let along_y = corner + vec2(0.0, inward.y * 3.0);
            let ends = [
                (along_x - vec2(0.0, inward.y), vec2(0.0, inward.y)),
                (along_y - vec2(inward.x, 0.0), vec2(inward.x, 0.0)),
            ];
            for (from, way) in ends {
                let [half] = reaches(&mesh, from, from + way * 2.0, [0.5]);
                assert!(
                    (half - 1.0).abs() <= 1.0 / 1024.0,
                    "the corner at {corner:?}: its arc's end {half} from {from:?}, not 1"
                );
            }
            let [half] = reaches(&mesh, corner, corner + inward * 1.5, [0.5]);
            let got = half * std::f32::consts::FRAC_1_SQRT_2;
            assert!(
                (got - diagonal).abs() <= 1.0 / 1024.0,
                "the corner at {corner:?}: its outer edge {got} in on its diagonal, not \
                 {diagonal}"
            );
        }
    }

    /// Each inner corner is a quarter ellipse about the point 3 in on each axis, its radii the
    /// outer's 3 less the border's widths: 2 and 2 at the top, 2 across and 1 down at the
    /// bottom. egui feathers an edge across it, along its normal, so at each of the quarter's
    /// points, (3 − rx cos φ, 3 − ry sin φ) in from the corner for φ in eighths of a right
    /// angle, the line along the ellipse's normal there, (cos φ / rx, sin φ / ry) out of the
    /// cap's inside, is half covered on the ellipse and feathered over [`RETINA_BAND`]. At the
    /// bottom, where the radii differ, that normal is not the way from the centre: at 22.5°,
    /// 39.6° from the side rather than 22.5°; a feather along the way from the centre is
    /// cos 17.1° = 0.956 as wide across the edge, 0.382.
    #[test]
    fn a_key_caps_inner_corners_are_feathered_along_their_normals() {
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(24.0, 16.0));
        let mesh = key_cap_border(rect, Color32::WHITE, RETINA_FEATHER);
        for (index, (corner, inward)) in corners(rect).into_iter().enumerate() {
            let (rx, ry) = if index < 2 { (2.0, 2.0) } else { (2.0, 1.0) };
            let centre = corner + inward * 3.0;
            for step in 1..8_u8 {
                let angle = std::f32::consts::FRAC_PI_2 * f32::from(step) / 8.0;
                let (cos, sin) = (libm::cosf(angle), libm::sinf(angle));
                let on = centre - inward * vec2(rx * cos, ry * sin);
                let normal = (-inward * vec2(cos / rx, sin / ry)).normalized();
                let (half, band) = edge(&mesh, on - normal * 0.5, on + normal * 0.5);
                assert!(
                    (half - 0.5).abs() <= 1.0 / 512.0 && (band - RETINA_BAND).abs() <= 1.0 / 512.0,
                    "the corner at {corner:?}, {step} eighths round: half covered {half} \
                     from inside its edge and feathered over {band}, not 0.5 and \
                     {RETINA_BAND}"
                );
            }
        }
    }
}
