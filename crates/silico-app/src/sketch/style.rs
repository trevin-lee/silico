//! Sketch mode's colours and sizes, the approved prototype's dark-theme values
//! (docs/design/m5-ui/DESIGN.md's *Tokens* and `js/sketch.js`; approved 2026-11-01). These
//! move into `theme.rs` when it lands with the port (#350): the first block under the names
//! it gives them, so the move is a delete; the rest are sketch mode's own, until `theme.rs`
//! names them too.

use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};

// ---- Under `theme.rs`'s names (#350) ----

/// An under-defined member (Blue.600).
pub const SKETCH_UNDER_DEFINED: Color32 = Color32::from_rgb(0x7a, 0x95, 0xff);
/// A fully defined member (Gray.900, SolidWorks' black on a dark canvas).
pub const SKETCH_FULLY_DEFINED: Color32 = Color32::from_rgb(0xee, 0xee, 0xed);
/// An over-defined member or constraint, and a sketch no algorithm solved (Red.550).
pub const SKETCH_OVER_DEFINED: Color32 = Color32::from_rgb(0xff, 0x38, 0x65);
/// A sketch solved only to geometry that can't be built.
pub const SKETCH_INVALID: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);
/// A relation to geometry that no longer exists. No member reads so yet (the gate refuses a
/// reference to a member that isn't there): it's for the model references to come.
pub const SKETCH_DANGLING: Color32 = Color32::from_rgb(0xb5, 0xb8, 0x4a);
/// A driven (reference) dimension (Gray.500).
pub const SKETCH_DRIVEN: Color32 = Color32::from_rgb(0x82, 0x7f, 0x7f);
/// Construction geometry's dash and gap, in points.
pub const SKETCH_CONSTRUCTION_DASH: [f32; 2] = [5.0, 3.0];
/// A curve's stroke, in points.
pub const SKETCH_STROKE_WIDTH: f32 = 1.8;
/// A selected member's stroke, and its width, over a wider halo.
pub const SKETCH_SELECTED_STROKE: Color32 = Color32::WHITE;
pub const SKETCH_SELECTED_WIDTH: f32 = 2.4;
pub const SKETCH_SELECTED_HALO_WIDTH: f32 = 6.0;
/// A dimension's value, and a tool's label beside the pointer, in points.
pub const CANVAS_TEXT_SIZE: f32 = 11.5;
pub const CANVAS_LABEL_SIZE: f32 = 11.0;

/// An input's frame (the Modify box's, M5.11c): a stand-in for `theme::field_frame`, of its
/// signature, until it lands.
pub fn field_frame(focused: bool, invalid: bool) -> egui::Frame {
    let stroke = match (invalid, focused) {
        (true, _) => SKETCH_OVER_DEFINED,
        (false, true) => FOCUS,
        (false, false) => GRAY_350,
    };
    egui::Frame::new()
        .fill(FLOATING_BG)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 2))
}

// ---- Sketch mode's own, until `theme.rs` names them ----

/// The selection halo under a selected member: Blue.525 at 35% (under `theme.rs`'s name).
pub const SKETCH_SELECTED_HALO: Color32 =
    Color32::from_rgba_unmultiplied_const(0x46, 0x76, 0xff, 89);
/// A relation repeating others, its glyph's colour: the warning colour, as `check` warns of
/// it (`constraints_redundant`), never the over-defined red, which it isn't.
pub const SKETCH_REDUNDANT: Color32 = Color32::from_rgb(0xff, 0x7a, 0x0c);
/// A driving dimension's lines and value (Gray.750, text).
pub const SKETCH_DIMENSION: Color32 = TEXT_DEFAULT;
/// A dimension's lines, in points.
pub const SKETCH_DIMENSION_WIDTH: f32 = 1.0;
/// How far a dimension's line stands from what it measures; where its extension lines
/// start off the geometry, and how far past the dimension line they run, in points.
pub const SKETCH_DIMENSION_OFFSET: f32 = 26.0;
pub const SKETCH_EXTENSION_GAP: f32 = 4.0;
pub const SKETCH_EXTENSION_PAST: f32 = 5.0;
/// A dimension's arrowheads: their length, and half their width, in points.
pub const SKETCH_ARROW_LENGTH: f32 = 7.0;
pub const SKETCH_ARROW_HALF_WIDTH: f32 = 3.0;
/// A dimension's value's box: its height, its padding each side, and its fill.
pub const SKETCH_LABEL_HEIGHT: f32 = 18.0;
pub const SKETCH_LABEL_PAD: f32 = 5.0;
pub const SKETCH_LABEL_FILL: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 217);
/// A relation's glyph: its box's side and corner, its fill, its border, and its icon's
/// colour and stroke, in points.
pub const SKETCH_GLYPH_SIDE: f32 = 16.0;
pub const SKETCH_GLYPH_RADIUS: u8 = 3;
pub const SKETCH_GLYPH_FILL: Color32 = FLOATING_BG;
pub const SKETCH_GLYPH_BORDER: Color32 = GRAY_350;
pub const SKETCH_GLYPH_ICON: Color32 = Color32::from_rgb(0xbf, 0xbf, 0xbf);
pub const SKETCH_GLYPH_ICON_WIDTH: f32 = 1.3;
/// Where a glyph stands: off its member, a share of a line's way along it, and how far
/// apart glyphs on one member stand, in points.
pub const SKETCH_GLYPH_OFFSET: f32 = 13.0;
pub const SKETCH_GLYPH_ALONG: f64 = 0.28;
pub const SKETCH_GLYPH_STEP: f32 = 18.0;
/// A point's square's side, in points (under `theme.rs`'s name).
pub const SKETCH_POINT_SIZE: f32 = 5.0;
/// The profile's fill (Show Profile): the selection's blue at 10% (under `theme.rs`'s name).
pub const SKETCH_PROFILE_FILL: Color32 =
    Color32::from_rgba_unmultiplied_const(0x18, 0x6a, 0xdd, 26);
/// The sketch's origin: a ring, its radius and stroke.
pub const SKETCH_ORIGIN: Color32 = SKETCH_DIMENSION;
pub const SKETCH_ORIGIN_RADIUS: f32 = 3.0;
pub const SKETCH_ORIGIN_WIDTH: f32 = 1.2;
/// The sketch's grid: its lines, each 50 mm's, and the axes' (theirs at 55%).
pub const SKETCH_GRID_MINOR: Color32 = Color32::from_rgb(0x17, 0x17, 0x17);
pub const SKETCH_GRID_MAJOR: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
pub const SKETCH_GRID_WIDTH: f32 = 1.0;
pub const SKETCH_AXIS_ALPHA: f32 = 0.55;
/// The model's axes' colours (`axis_color_x/y/z`).
pub const AXIS_X: Color32 = Color32::from_rgb(0xff, 0x69, 0x81);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x00, 0xb2, 0x66);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x7a, 0x95, 0xff);
/// A tool's entity under way: dashed, in the under-defined blue.
pub const SKETCH_PREVIEW: Color32 = SKETCH_UNDER_DEFINED;
pub const SKETCH_PREVIEW_WIDTH: f32 = 1.6;
/// A dotted inference line, and the glyph of a relation the pointer shows (Blue.525).
pub const SKETCH_GUIDE: Color32 = FOCUS;
pub const SKETCH_GUIDE_DASH: [f32; 2] = [2.0, 3.0];
/// A snap onto a point: a ring of this radius and stroke.
pub const SKETCH_SNAP: Color32 = SKETCH_INVALID;
pub const SKETCH_SNAP_RADIUS: f32 = 6.0;
pub const SKETCH_SNAP_WIDTH: f32 = 1.5;
/// The pointer's crosshair while a tool is on: its arms' length.
pub const SKETCH_CROSSHAIR: Color32 = Color32::WHITE;
pub const SKETCH_CROSSHAIR_ARM: f32 = 8.0;
/// Where a shown relation's glyph stands from the pointer, and a tool's label.
pub const SKETCH_SHOWN_AT: egui::Vec2 = egui::Vec2::new(18.0, -16.0);
pub const SKETCH_TOOL_LABEL_AT: egui::Vec2 = egui::Vec2::new(14.0, 12.0);
/// How far a curve's polyline may stray from the curve, and how near the pointer picks, in
/// points.
pub const SKETCH_CHORD_TOLERANCE: f32 = 0.25;
pub const SKETCH_PICK_TOLERANCE: f32 = 7.0;

/// The Sketch Palette: its width, where it stands from the canvas's top right, and its
/// title row's height.
pub const PALETTE_WIDTH: f32 = 212.0;
pub const PALETTE_FROM_CORNER: egui::Vec2 = egui::Vec2::new(-12.0 - PALETTE_WIDTH, 118.0);
pub const PALETTE_TITLE_HEIGHT: f32 = 26.0;
/// A section's heading's size, and the palette's small text's.
pub const PALETTE_HEAD_SIZE: f32 = 10.0;
pub const PALETTE_SMALL_SIZE: f32 = 11.0;
/// The in-canvas sketch bar: where it stands below the canvas's top.
pub const SKETCH_BAR_FROM_TOP: f32 = 10.0;
/// A state's dot's radius.
pub const STATE_DOT_RADIUS: f32 = 4.0;

/// re_ui's tokens the palette is drawn with, under `theme.rs`'s names where it gives them
/// (FLOATING_BG, PANEL_BORDER, TEXT_SUBDUED, TEXT_DEFAULT, TEXT_STRONG, FOCUS): floating
/// panels' fill (Gray.200), their border (Gray.250), strong strokes (Gray.350), the text's
/// shades, the selection's blue (Blue.500), and the focus blue (Blue.525).
pub const FLOATING_BG: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
pub const PANEL_BORDER: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
pub const GRAY_350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
pub const TEXT_SUBDUED: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
pub const TEXT_DEFAULT: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
pub const TEXT_STRONG: Color32 = Color32::WHITE;
pub const SELECTION: Color32 = Color32::from_rgb(0x18, 0x6a, 0xdd);
pub const FOCUS: Color32 = Color32::from_rgb(0x46, 0x76, 0xff);
