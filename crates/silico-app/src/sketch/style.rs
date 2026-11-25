//! Sketch mode's own values, the approved prototype's (docs/design/m5-ui/: DESIGN.md's
//! *Tokens*, `js/sketch.js`): what nothing else in the app draws or decides with, the pick
//! and chord tolerances, the drag threshold, the ghost, the Modify box, and the over-define
//! prompt. Its colours and the sizes it shares with the rest of the app (the definition
//! states' colours, the canvas's strokes, glyphs, and dimensions, the grid, the axes) are
//! the theme's, in `crate::theme` alone, and re-exported here, so sketch mode's files take
//! both from this module. The app is dark only, as the prototype is.

use eframe::egui::{Color32, Margin};

use crate::theme;
pub use crate::theme::{
    AXIS_X, AXIS_Y, AXIS_Z, CANVAS_LABEL_SIZE, CANVAS_TEXT_SIZE, ERROR_TEXT, FLOATING_BG, FOCUS,
    FieldKind, GRAY_350, GRAY_700, ON_SELECTION, PANEL_BORDER, SELECTION_BG,
    SKETCH_CONSTRUCTION_DASH, SKETCH_CROSSHAIR_STROKE, SKETCH_DANGLING, SKETCH_DIMENSION_ARROW,
    SKETCH_DIMENSION_GAP, SKETCH_DIMENSION_LABEL_BG, SKETCH_DIMENSION_LABEL_PADDING,
    SKETCH_DIMENSION_OFFSET, SKETCH_DIMENSION_TEXT, SKETCH_DRIVEN, SKETCH_FULLY_DEFINED,
    SKETCH_GLYPH_BG, SKETCH_GLYPH_OFFSET, SKETCH_GLYPH_SIZE, SKETCH_GLYPH_STROKE,
    SKETCH_GUIDE_DASH, SKETCH_GUIDE_STROKE, SKETCH_INVALID, SKETCH_ORIGIN_RING,
    SKETCH_ORIGIN_RING_RADIUS, SKETCH_OVER_DEFINED, SKETCH_PALETTE_WIDTH, SKETCH_POINT_SIZE,
    SKETCH_PREVIEW_STROKE, SKETCH_PROFILE_FILL, SKETCH_REDUNDANT, SKETCH_SELECTED_HALO,
    SKETCH_SELECTED_HALO_WIDTH, SKETCH_SELECTED_STROKE, SKETCH_SELECTED_WIDTH, SKETCH_SNAP_STROKE,
    SKETCH_STROKE_WIDTH, SKETCH_UNDER_DEFINED, TEXT_DEFAULT, TEXT_STRONG, TEXT_SUBDUED,
    VIEWPORT_GRID_AXIS_WIDTH, VIEWPORT_GRID_FADE_MM, VIEWPORT_GRID_MAJOR, VIEWPORT_GRID_MAJOR_MM,
    VIEWPORT_GRID_MINOR, VIEWPORT_GRID_MINOR_MM, WARNING, color32, field_frame,
};

// ---- Sketch mode's own ----

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

/// The Modify box (the prototype's `.modify-box`): its frame's fill, a floating panel's
/// (Gray.200), and border (Blue.500), and its field's width (its field is the theme's
/// [`field_frame`] of [`FieldKind::Modify`]). Its hint's size; and the over-define prompt's
/// width (the prototype's `.modal`, a modal's), its choices' border (Gray.250), and the keys'
/// fill (Gray.250), border (Gray.350), height, and text (Gray.800), a key cap's (`.kbd`).
pub const SKETCH_MODIFY_BG: Color32 = theme::FLOATING_BG;
pub const SKETCH_MODIFY_STROKE: Color32 = theme::SELECTION_BG;
pub const SKETCH_MODIFY_FIELD_WIDTH: f32 = 72.0;
pub const SKETCH_MODIFY_HINT_SIZE: f32 = 10.5;
/// The Modify box's ✓ and ✕ buttons' side (the prototype's `.ab-btn`, `--btn`).
pub const MODIFY_BUTTON_SIZE: f32 = 22.0;
/// The most characters the Modify box's field takes: far past any number a document
/// writes, so nothing typed there is cut, while a paste of a page isn't held.
pub const MODIFY_CHAR_LIMIT: usize = 256;
pub const SKETCH_PROMPT_WIDTH: f32 = theme::MODAL_WIDTH;
/// How far the Modify box keeps from the Sketch Palette when it's moved off it, in points
/// (the prototype's gap between floating panels).
pub const SKETCH_PALETTE_CLEARANCE: f32 = 8.0;
pub const SKETCH_PROMPT_CHOICE_STROKE: Color32 = theme::PANEL_BORDER;
pub const SKETCH_KEY_BG: Color32 = theme::KEY_CAP_BG;
pub const SKETCH_KEY_STROKE: Color32 = theme::KEY_CAP_STROKE;
/// The over-define prompt's padding (the prototype's `.m-head` and `.m-body`: 16 at its
/// sides, 12 above and below), the gap between its choices (`.choice`'s margin), each
/// choice's padding (8 above and below, 10 at the sides) and the gap between its words and
/// its key; a key's badge's height (`.kbd`, its border inside it) and text (Gray.800).
pub const SKETCH_PROMPT_MARGIN: Margin = Margin {
    left: 16,
    right: 16,
    top: 12,
    bottom: 12,
};
pub const SKETCH_PROMPT_CHOICE_GAP: f32 = 6.0;
pub const SKETCH_PROMPT_CHOICE_PADDING: Margin = Margin::symmetric(10, 8);
pub const SKETCH_PROMPT_CHOICE_GAP_X: f32 = 10.0;
pub const SKETCH_KEY_HEIGHT: f32 = theme::KEY_CAP_HEIGHT;
pub const SKETCH_KEY_TEXT: Color32 = theme::KEY_CAP_TEXT;
