//! Sketch mode's canvas colours and sizes, the approved prototype's (docs/design/m5-ui/:
//! DESIGN.md's *Tokens*, `js/sketch.js`). They move into `theme.rs` with the port: the
//! shared tokens here are stand-ins under `theme.rs`'s own names, types, and values, so the
//! move deletes them; the sketch's own follow, under the names agreed for it. The app is
//! dark only, as the prototype is.

use eframe::egui::{self, Color32, Margin, Stroke};

// ---- Stand-ins for `theme.rs`'s shared tokens: its names, types, and values ----

/// A colour as the renderer takes it: sRGB red, green, blue, and alpha (not premultiplied).
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

/// An opaque [`Rgba8`] from its hex value.
const fn rgba8(rgb: u32) -> Rgba8 {
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b, 0xff]
}

/// Gray.350, and Gray.700: a glyph's icon.
pub const GRAY_350: Color32 = hex(0x525151);
pub const GRAY_700: Color32 = hex(0xbfbfbf);
/// The focus blue (Blue.525): a guide, and the relation the pointer shows.
pub const FOCUS: Color32 = hex(0x4676ff);
/// The warning orange.
pub const WARNING: Color32 = hex(0xff7a0c);
/// The panels' and text's tokens the palette and the sketch bar are drawn with: floating
/// panels' fill (Gray.200) and border (Gray.250); subdued (Gray.550), default (Gray.750),
/// and strong (Gray.1000) text; text on the selection (Blue.900); and error text (Red.550).
pub const FLOATING_BG: Color32 = hex(0x212121);
pub const PANEL_BORDER: Color32 = hex(0x272626);
pub const TEXT_SUBDUED: Color32 = hex(0x939090);
pub const TEXT_DEFAULT: Color32 = hex(0xcfcfcf);
pub const TEXT_STRONG: Color32 = hex(0xffffff);
pub const ON_SELECTION: Color32 = hex(0xf0f2ff);
pub const ERROR_TEXT: Color32 = hex(0xff3865);
/// The selection's blue (Blue.500): the profile's fill is it, faint.
pub const SELECTION_BG: Color32 = hex(0x186add);
/// The axes: X (Red.600), Y (Green.600), Z (Blue.600).
pub const AXIS_X: Rgba8 = rgba8(0xff6981);
pub const AXIS_Y: Rgba8 = rgba8(0x00b266);
pub const AXIS_Z: Rgba8 = rgba8(0x7a95ff);
/// The grid's minor lines, `#3a3939` at 75%, and its major ones, Gray.400 at 90%.
pub const VIEWPORT_GRID_MINOR: Rgba8 = [0x3a, 0x39, 0x39, 191];
pub const VIEWPORT_GRID_MAJOR: Rgba8 = [0x64, 0x63, 0x63, 230];
/// The minor and major lines' spacing, in millimetres.
pub const VIEWPORT_GRID_MINOR_MM: f32 = 10.0;
pub const VIEWPORT_GRID_MAJOR_MM: f32 = 50.0;
/// How far from the origin the grid has faded out, in millimetres: linearly, from full at
/// the origin.
pub const VIEWPORT_GRID_FADE_MM: f32 = 200.0;
/// The grid's axis lines' width, each in its axis's colour.
pub const VIEWPORT_GRID_AXIS_WIDTH: f32 = 1.5;

/// Under defined (Blue.600).
pub const SKETCH_UNDER_DEFINED: Color32 = hex(0x7a95ff);
/// Fully defined (Gray.900: SolidWorks' black on a dark canvas).
pub const SKETCH_FULLY_DEFINED: Color32 = hex(0xeeeeed);
/// Over defined, and unsolvable.
pub const SKETCH_OVER_DEFINED: Color32 = hex(0xff3865);
/// Invalid geometry.
pub const SKETCH_INVALID: Color32 = hex(0xf2c94c);
/// Dangling. No member reads so yet (the gate refuses a reference to a member that isn't
/// there): it's for the model references to come.
pub const SKETCH_DANGLING: Color32 = hex(0xb5b84a);
/// A driven dimension (Gray.500).
pub const SKETCH_DRIVEN: Color32 = hex(0x827f7f);
/// A curve's stroke.
pub const SKETCH_STROKE_WIDTH: f32 = 1.8;
/// Construction geometry's dash and gap, in points.
pub const SKETCH_CONSTRUCTION_DASH: [f32; 2] = [5.0, 3.0];
/// A selected entity's stroke, and its width, over a halo of this colour and width.
pub const SKETCH_SELECTED_STROKE: Color32 = hex(0xffffff);
pub const SKETCH_SELECTED_WIDTH: f32 = 2.4;
pub const SKETCH_SELECTED_HALO: Color32 = hex_alpha(0x4676ff, 89);
pub const SKETCH_SELECTED_HALO_WIDTH: f32 = 6.0;
/// A sketch point's square's side.
pub const SKETCH_POINT_SIZE: f32 = 5.0;
/// A closed profile's fill: Blue.500 at 10%.
pub const SKETCH_PROFILE_FILL: Color32 = hex_alpha(0x186add, 26);
/// A dimension's text's size, and other text on the canvas (a length while drawing).
pub const CANVAS_TEXT_SIZE: f32 = 11.5;
pub const CANVAS_LABEL_SIZE: f32 = 11.0;

// ---- The sketch's own ----

/// A relation repeating others, its glyph's colour: the warning orange, as `check` warns of
/// it (`constraints_redundant`), never the over-defined red, which it isn't.
pub const SKETCH_REDUNDANT: Color32 = WARNING;
/// A driving dimension's lines and value (Gray.750).
pub const SKETCH_DIMENSION_TEXT: Color32 = hex(0xcfcfcf);
/// How far a dimension's line stands from what it measures, in points.
pub const SKETCH_DIMENSION_OFFSET: f32 = 26.0;
/// How far its extension lines start off the geometry, and run past its line, in points.
pub const SKETCH_DIMENSION_GAP: f32 = 4.0;
/// Its arrowheads' length, then width, in points.
pub const SKETCH_DIMENSION_ARROW: [f32; 2] = [7.0, 6.0];
/// Its value's box: black at 85%, padded so about the text (18 high, its corners 3).
pub const SKETCH_DIMENSION_LABEL_BG: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 217);
pub const SKETCH_DIMENSION_LABEL_PADDING: Margin = Margin::symmetric(5, 3);
/// A relation glyph's box: its side, fill, and border (its icon in [`GRAY_700`]); and how
/// far it stands from its member, in points.
pub const SKETCH_GLYPH_SIZE: f32 = 16.0;
pub const SKETCH_GLYPH_BG: Color32 = hex(0x212121);
pub const SKETCH_GLYPH_STROKE: Color32 = GRAY_350;
pub const SKETCH_GLYPH_OFFSET: f32 = 13.0;
/// The sketch's origin: a ring, and its radius.
pub const SKETCH_ORIGIN_RING: Stroke = Stroke {
    width: 1.2,
    color: hex(0xcfcfcf),
};
pub const SKETCH_ORIGIN_RING_RADIUS: f32 = 3.0;
/// A tool's entity under way, dashed as construction is.
pub const SKETCH_PREVIEW_STROKE: Stroke = Stroke {
    width: 1.6,
    color: SKETCH_UNDER_DEFINED,
};
/// A dotted inference line, and a relation the pointer shows (Blue.525).
pub const SKETCH_GUIDE_STROKE: Stroke = Stroke {
    width: 1.0,
    color: FOCUS,
};
pub const SKETCH_GUIDE_DASH: [f32; 2] = [2.0, 3.0];
/// A snap onto a point: its ring.
pub const SKETCH_SNAP_STROKE: Stroke = Stroke {
    width: 1.5,
    color: SKETCH_INVALID,
};
/// The pointer's crosshair while a tool is on.
pub const SKETCH_CROSSHAIR_STROKE: Stroke = Stroke {
    width: 1.0,
    color: hex(0xffffff),
};
/// The Sketch Palette's width, and the sketch bar's height, in points.
pub const SKETCH_PALETTE_WIDTH: f32 = 212.0;
pub const SKETCH_BAR_HEIGHT: f32 = 32.0;
/// How far a curve's polyline may stray from the curve, and how near the pointer picks, in
/// points.
pub const SKETCH_CHORD_TOLERANCE: f32 = 0.25;
pub const SKETCH_PICK_TOLERANCE: f32 = 7.0;

// ---- M5.11c: Smart Dimension, the over-define prompt, and drags ----

/// A drag's ghost (section 9, *Ghost image on drag*): the begin state drawn at this share
/// of its colour.
pub const SKETCH_GHOST_OPACITY: f32 = 0.35;
/// How far a press must move, in points, before it's a drag rather than a click: behaviour,
/// which stays here.
pub const SKETCH_DRAG_THRESHOLD: f32 = 3.0;

/// The Modify box (the prototype's `.modify-box`): its frame's fill (Gray.200) and border
/// (Blue.500), and its field's width; a field's fill (Gray.150, a dialog's Gray.200, which
/// Gray.250 when hovered), border (Gray.350; none in a dialog), its border focused (the focus
/// blue), and invalid (Red.550). Its hint's size; and the over-define prompt's width (the
/// prototype's `.modal`), its choices' border (Gray.250), and the keys' fill (Gray.250) and
/// border (Gray.350).
pub const SKETCH_MODIFY_BG: Color32 = hex(0x212121);
pub const SKETCH_MODIFY_STROKE: Color32 = SELECTION_BG;
pub const SKETCH_MODIFY_FIELD_WIDTH: f32 = 72.0;
pub const FIELD_BG: Color32 = hex(0x212121);
pub const FIELD_BG_HOVERED: Color32 = hex(0x272626);
pub const MODIFY_FIELD_BG: Color32 = hex(0x171717);
pub const MODIFY_FIELD_STROKE: Color32 = GRAY_350;
pub const SKETCH_MODIFY_HINT_SIZE: f32 = 10.5;
/// The Modify box's ✓ and ✕ buttons' side (the prototype's `.ab-btn`, `--btn`).
pub const MODIFY_BUTTON_SIZE: f32 = 22.0;
/// The most characters the Modify box's field takes: far past any number a document
/// writes, so nothing typed there is cut, while a paste of a page isn't held.
pub const MODIFY_CHAR_LIMIT: usize = 256;
pub const SKETCH_PROMPT_WIDTH: f32 = 440.0;
/// How far the Modify box keeps from the Sketch Palette when it's moved off it, in points
/// (the prototype's gap between floating panels).
pub const SKETCH_PALETTE_CLEARANCE: f32 = 8.0;
pub const SKETCH_PROMPT_CHOICE_STROKE: Color32 = PANEL_BORDER;
pub const SKETCH_KEY_BG: Color32 = PANEL_BORDER;
pub const SKETCH_KEY_STROKE: Color32 = GRAY_350;

/// Which field a frame is for: a feature dialog's, or the Modify box's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Dialog,
    Modify,
}

/// A field's frame, as the prototype draws one (`.field`, `.modify-box input`): a stand-in
/// of `theme::field_frame`'s exact signature, until the theme lands, which this then
/// becomes. Its fill by its kind (a dialog's lighter when `hovered`); its border the focus
/// blue when `focused`, red when `invalid` (which wins), else its kind's.
pub fn field_frame(kind: FieldKind, hovered: bool, focused: bool, invalid: bool) -> egui::Frame {
    let (fill, rest) = match kind {
        FieldKind::Dialog if hovered && !focused => (FIELD_BG_HOVERED, Color32::TRANSPARENT),
        FieldKind::Dialog => (FIELD_BG, Color32::TRANSPARENT),
        FieldKind::Modify => (MODIFY_FIELD_BG, MODIFY_FIELD_STROKE),
    };
    let border = if invalid {
        ERROR_TEXT
    } else if focused {
        FOCUS
    } else {
        rest
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 2))
}
