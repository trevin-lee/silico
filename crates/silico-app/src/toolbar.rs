//! The toolbar, as the approved UI prototype draws it (docs/design/m5-ui/DESIGN.md, *Layout*
//! and *egui mapping*; styles.css's `.tb-*` and `.btn-finish`; app.js's `renderToolbar` and
//! its command table, interact.js's `createMenu`, `modifyMenu`, and `selectMenu`): a row of
//! tabs, the Design workspace's SOLID and, while a sketch is open, the contextual SKETCH, in
//! blue; under it the tab's panels, each a row of icon buttons over its capital label, a
//! drop-down; and, in SKETCH, a blue Finish Sketch at the row's right end.
//!
//! Only what the app can do is on: the feature dialogs (M5.8), SELECT's filters and
//! clearing the selection (M5.7), and, in SKETCH, sketch mode's tools (the drawing tools,
//! trim, split, and Smart Dimension, the one on drawn on, `.tb-btn.on`), Construction
//! (drawn pressed while on, `.toggle-on`, as the filters are), and the ten constraint tools,
//! each a relation made of the selection. A dialog's command that's off for want of what the
//! part has says why in its tooltip (a boolean's "Needs two bodies"). The prototype's other
//! commands keep their places, drawn off, their tooltips saying they aren't built yet. What
//! the toolbar shows comes in a [`Shown`], and what was clicked goes back as an [`Act`], so a
//! test drives it with a stub, and sketch mode's state (whether a sketch is open, whether
//! Create Sketch can open, the tool on, Construction) and its actions (Create Sketch on a
//! plane, a tool, Construction, a relation, Finish Sketch) are the caller's to wire.
//! A button the registry (`crate::shortcuts`) gives keys names them in its tooltip.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, FontId, Galley, Layout, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, accesskit::Role, text::LayoutJob,
};
use silico_core::document::OriginPlane;
use silico_core::sketch_tools::{RelationTool, ToolKind};

use crate::bar;
use crate::dialog::Kind;
use crate::icons::{self, ICON_SIZE, Icon, IconSize, TOOLBAR_ICON_SIZE};
use crate::selection::Filter;
use crate::shortcuts;
use crate::sketch::tools::Choice;
use crate::theme;

/// What the toolbar shows: the caller's state, which a test stubs.
pub struct Shown<'a> {
    /// Whether a sketch is open: the SKETCH tab, its panels, and Finish Sketch.
    pub in_sketch: bool,
    /// Whether Create Sketch can open now, or `None` while the app has no sketch mode: the
    /// button is then off, its tooltip saying so.
    pub create_sketch: Option<bool>,
    /// Whether a feature's dialog of each kind can open now.
    pub can_open: &'a dyn Fn(Kind) -> bool,
    /// Why a feature's dialog of each kind can't open, if the part lacks what it needs
    /// (a boolean's two bodies): its button's tooltip says it while it's off.
    pub why_closed: &'a dyn Fn(Kind) -> Option<&'static str>,
    /// Whether anything is selected, for SELECT's Clear selection.
    pub selected: bool,
    /// In a sketch, the tool on, if one is.
    pub tool: Option<Choice>,
    /// In a sketch, whether Construction is on.
    pub construction: bool,
    /// The selection's filter, which SELECT's buttons show and toggle.
    pub filter: Filter,
}

/// A kind of element the selection's filter takes or leaves (SELECT's buttons; X, E, V).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Faces,
    Edges,
    Vertices,
}

impl FilterKind {
    /// Whether `filter` takes it.
    pub fn on(self, filter: Filter) -> bool {
        match self {
            FilterKind::Faces => filter.faces,
            FilterKind::Edges => filter.edges,
            FilterKind::Vertices => filter.vertices,
        }
    }

    /// `filter` with it toggled (M5.0, open item 24: each toggles its kind in the set).
    pub fn toggled(self, mut filter: Filter) -> Filter {
        match self {
            FilterKind::Faces => filter.faces = !filter.faces,
            FilterKind::Edges => filter.edges = !filter.edges,
            FilterKind::Vertices => filter.vertices = !filter.vertices,
        }
        filter
    }
}

/// What was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// A feature's dialog, to open.
    Dialog(Kind),
    /// Create Sketch, on an origin plane.
    CreateSketch(OriginPlane),
    /// Finish Sketch.
    FinishSketch,
    /// A sketch tool's button: the tool chosen, or left if it's on.
    SketchTool(Choice),
    /// Construction's button: Construction turned on or off.
    Construction,
    /// A constraint tool's button: the relation made of the selection.
    Relate(RelationTool),
    /// SELECT's Clear selection.
    ClearSelection,
    /// One of SELECT's filters, toggled.
    Filter(FilterKind),
    /// SELECT's Select all filters: every kind taken again.
    AllFilters,
}

/// The toolbar (`#toolbar`) as a panel at the top of `ui`, showing `shown`: its tabs, its
/// tab's panels, and Finish Sketch in a sketch. What was clicked, if anything.
pub fn show(ui: &mut Ui, shown: &Shown<'_>) -> Option<Act> {
    let mut act = None;
    egui::Panel::top("toolbar")
        .exact_size(HEIGHT)
        .frame(egui::Frame::NONE.fill(theme::TOOLBAR_BG))
        .show_separator_line(false)
        .show(ui, |ui| {
            let bar = ui.max_rect();
            ui.painter().hline(
                bar.x_range(),
                bar.bottom() - 0.5,
                Stroke::new(theme::BORDER_WIDTH, theme::TOOLBAR_BORDER),
            );
            let tabs =
                Rect::from_min_size(bar.min, Vec2::new(bar.width(), theme::TOOLBAR_TABS_HEIGHT));
            let panels = Rect::from_min_size(
                tabs.left_bottom(),
                Vec2::new(bar.width(), theme::TOOLBAR_PANELS_HEIGHT),
            );
            tabs_row(ui, tabs, shown.in_sketch);
            panels_row(ui, panels, shown, &mut act);
        });
    act
}

/// The toolbar's height: its tabs, its panels, and its border below them.
const HEIGHT: f32 = theme::TOOLBAR_TABS_HEIGHT + theme::TOOLBAR_PANELS_HEIGHT + theme::BORDER_WIDTH;

// ---- The commands (app.js's `CMD`) ----

/// What a command's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Does {
    /// Opens a feature's dialog.
    Dialog(Kind),
    /// Opens Create Sketch's planes.
    CreateSketch,
    /// Chooses a sketch tool, or leaves it if it's on.
    Tool(Choice),
    /// Turns Construction on or off.
    Construction,
    /// Toggles a kind in the selection's filter, shown pressed while it's taken.
    Filter(FilterKind),
    /// Makes a relation of sketch mode's selection: on whenever a sketch is open, as the
    /// prototype draws the constraint tools.
    Constraint(RelationTool),
    /// Nothing yet: drawn off.
    NotBuilt,
}

/// A command as the toolbar offers it: its name (its button's label and its tooltip's
/// title), its icon, the prototype's line on it, what it does, and the registry's command
/// whose keys its tooltip names, if it has one.
#[derive(Debug, Clone, Copy)]
struct Command {
    label: &'static str,
    icon: Icon,
    description: Option<&'static str>,
    does: Does,
    key: Option<shortcuts::Command>,
}

const fn command(
    label: &'static str,
    icon: Icon,
    description: Option<&'static str>,
    does: Does,
) -> Command {
    Command {
        label,
        icon,
        description,
        does,
        key: None,
    }
}

/// A command the registry's `key` reaches too, its tooltip naming its keys.
const fn keyed(command: Command, key: shortcuts::Command) -> Command {
    Command {
        key: Some(key),
        ..command
    }
}

/// What a command's tooltip says when it isn't built yet.
const NOT_BUILT: &str = "Not built yet";

/// Create Sketch's words: its button's, and its item's in the canvas's context menu
/// (`crate::canvas_menu`), which opens the same planes ([`planes`]).
pub(crate) const CREATE_SKETCH_WORDS: &str = "Create Sketch";

const CREATE_SKETCH: Command = command(
    CREATE_SKETCH_WORDS,
    Icon::Sketch,
    // The prototype's "Pick an origin plane or a flat face, then sketch on it", once faces
    // are picked (M5.7).
    Some("Sketches on an origin plane"),
    Does::CreateSketch,
);
const PAD: Command = command(
    "Pad",
    Icon::Pad,
    Some("Extrudes a sketch's profile into solid"),
    Does::Dialog(Kind::Pad),
);
const POCKET: Command = command(
    "Pocket",
    Icon::Pocket,
    Some("Cuts a sketch's profile out of the solid"),
    Does::Dialog(Kind::Pocket),
);
const REVOLVE: Command = command(
    "Revolve",
    Icon::Revolve,
    Some("Sweeps a profile around an axis"),
    Does::Dialog(Kind::Revolve),
);
const GROOVE: Command = command(
    "Groove",
    Icon::Groove,
    Some("Cuts a profile swept around an axis"),
    Does::Dialog(Kind::Groove),
);
const FILLET: Command = command(
    "Fillet",
    Icon::Fillet,
    Some("Rounds the selected edges"),
    Does::Dialog(Kind::Fillet),
);
const CHAMFER: Command = command(
    "Chamfer",
    Icon::Chamfer,
    Some("Bevels the selected edges"),
    Does::Dialog(Kind::Chamfer),
);
const UNION: Command = command("Union", Icon::Union, None, Does::Dialog(Kind::Union));
const SUBTRACT: Command = command(
    "Subtract",
    Icon::Subtract,
    None,
    Does::Dialog(Kind::Subtract),
);
const INTERSECT: Command = command(
    "Intersect",
    Icon::Intersect,
    None,
    Does::Dialog(Kind::Intersect),
);
const MEASURE: Command = command(
    "Measure",
    Icon::Measure,
    Some("Distance, angle, and size of what you select"),
    Does::NotBuilt,
);
const FACES: Command = command(
    "Faces",
    Icon::SelFace,
    Some("Toggles whether faces can be selected"),
    Does::Filter(FilterKind::Faces),
);
const EDGES: Command = command(
    "Edges",
    Icon::SelEdge,
    Some("Toggles whether edges can be selected"),
    Does::Filter(FilterKind::Edges),
);
const VERTICES: Command = command(
    "Vertices",
    Icon::SelVertex,
    Some("Toggles whether vertices can be selected"),
    Does::Filter(FilterKind::Vertices),
);
const LINE: Command = keyed(
    command(
        "Line",
        Icon::Line,
        Some("Click, click: a chain of lines. Double-click ends it"),
        Does::Tool(Choice::Draw(ToolKind::Line)),
    ),
    shortcuts::Command::Line,
);
/// The corner rectangle, Fusion's and SolidWorks' first.
const RECTANGLE: Command = keyed(
    command(
        "Rectangle",
        Icon::Rect,
        None,
        Does::Tool(Choice::Draw(ToolKind::CornerRectangle)),
    ),
    shortcuts::Command::Rectangle,
);
const CIRCLE: Command = keyed(
    command(
        "Circle",
        Icon::Circle,
        None,
        Does::Tool(Choice::Draw(ToolKind::Circle)),
    ),
    shortcuts::Command::Circle,
);
/// The 3-point arc, Fusion's first, as A chooses it.
const ARC: Command = keyed(
    command(
        "Arc",
        Icon::Arc,
        None,
        Does::Tool(Choice::Draw(ToolKind::ThreePointArc)),
    ),
    shortcuts::Command::Arc,
);
const POINT: Command = keyed(
    command(
        "Point",
        Icon::Point,
        None,
        Does::Tool(Choice::Draw(ToolKind::Point)),
    ),
    shortcuts::Command::Point,
);
/// Construction, as the prototype has it: the selected curves switched between profile and
/// construction (`Tab::sketch_construction`); with no curve selected, the palette's setting
/// for the curves drawn next. Shown pressed while that setting is on.
const CONSTRUCTION: Command = keyed(
    command(
        "Construction",
        Icon::Construction,
        Some(shortcuts::CONSTRUCTION_HELP),
        Does::Construction,
    ),
    shortcuts::Command::Construction,
);
const TRIM: Command = keyed(
    command("Trim", Icon::Trim, None, Does::Tool(Choice::Trim)),
    shortcuts::Command::Trim,
);
/// No tool of its own: Trim's drag along a curve extends it.
const EXTEND: Command = command(
    "Extend",
    Icon::Extend,
    Some("Trim extends a curve dragged along"),
    Does::NotBuilt,
);
const SPLIT: Command = command("Split", Icon::Split, None, Does::Tool(Choice::Split));
const DIMENSION: Command = keyed(
    command(
        "Smart Dimension",
        Icon::Dimension,
        Some("Pick a line, a circle, or two entities, then type the value"),
        Does::Tool(Choice::Dimension),
    ),
    shortcuts::Command::Dimension,
);

/// A constraint's button (app.js's `renderToolbar`, CONSTRAINTS): its name, the prototype's
/// line on them all, and the relation it makes.
const fn constraint(label: &'static str, icon: Icon, relation: RelationTool) -> Command {
    command(
        label,
        icon,
        Some("Select the curves or points first"),
        Does::Constraint(relation),
    )
}

/// What a drop-down holds.
#[derive(Debug, Clone, Copy)]
enum Entry {
    Command(Command),
    Separator,
    /// A heading (`.mh`), in capitals.
    Heading(&'static str),
    /// Every selection filter on.
    AllFilters,
    /// The selection cleared.
    ClearSelection,
}

/// A panel: its label, its row of buttons, and its drop-down: its own entries, or, with
/// none, its row's commands.
struct Panel {
    label: &'static str,
    row: &'static [Command],
    menu: Option<&'static [Entry]>,
}

/// SOLID's panels (app.js's `renderToolbar`; interact.js's `createMenu`, `modifyMenu`, and
/// `selectMenu`). CREATE makes solids sketch-first, as
/// SolidWorks does (Trevin's decision of 2026-11-09, docs/design/m5-ui/DESIGN.md): Create
/// Sketch and the sweeps, and no primitive; one a file holds is edited, from Edit Feature.
const SOLID: &[Panel] = &[
    Panel {
        label: "CREATE",
        row: &[CREATE_SKETCH, PAD, POCKET, REVOLVE, GROOVE],
        menu: Some(&[
            Entry::Command(CREATE_SKETCH),
            Entry::Separator,
            Entry::Command(PAD),
            Entry::Command(POCKET),
            Entry::Command(REVOLVE),
            Entry::Command(GROOVE),
        ]),
    },
    Panel {
        label: "MODIFY",
        row: &[FILLET, CHAMFER, UNION, SUBTRACT, INTERSECT],
        menu: Some(&[
            Entry::Command(FILLET),
            Entry::Command(CHAMFER),
            Entry::Separator,
            Entry::Heading("COMBINE"),
            Entry::Command(UNION),
            Entry::Command(SUBTRACT),
            Entry::Command(INTERSECT),
        ]),
    },
    Panel {
        label: "INSPECT",
        row: &[MEASURE],
        menu: None,
    },
    Panel {
        label: "SELECT",
        row: &[FACES, EDGES, VERTICES],
        menu: Some(&[
            Entry::Heading("SELECTION FILTERS"),
            Entry::Command(FACES),
            Entry::Command(EDGES),
            Entry::Command(VERTICES),
            Entry::Separator,
            Entry::AllFilters,
            Entry::ClearSelection,
        ]),
    },
];

/// SKETCH's panels (app.js's `renderToolbar`, in a sketch), before Finish Sketch.
const SKETCH: &[Panel] = &[
    Panel {
        label: "CREATE",
        row: &[LINE, RECTANGLE, CIRCLE, ARC, POINT, CONSTRUCTION],
        menu: None,
    },
    Panel {
        label: "MODIFY",
        row: &[TRIM, EXTEND, SPLIT],
        menu: None,
    },
    Panel {
        label: "CONSTRAINTS",
        row: &CONSTRAINTS,
        menu: None,
    },
    Panel {
        label: "INSPECT",
        row: &[MEASURE],
        menu: None,
    },
];

/// CONSTRAINTS' row: Smart Dimension, then the constraints in the order app.js's
/// `renderToolbar` gives them.
const CONSTRAINTS: [Command; 11] = [
    DIMENSION,
    constraint("Horizontal", Icon::CHorizontal, RelationTool::Horizontal),
    constraint("Vertical", Icon::CVertical, RelationTool::Vertical),
    constraint("Coincident", Icon::CCoincident, RelationTool::Coincident),
    constraint("Tangent", Icon::CTangent, RelationTool::Tangent),
    constraint("Equal", Icon::CEqual, RelationTool::Equal),
    constraint("Parallel", Icon::CParallel, RelationTool::Parallel),
    constraint(
        "Perpendicular",
        Icon::CPerpendicular,
        RelationTool::Perpendicular,
    ),
    constraint("Midpoint", Icon::CMidpoint, RelationTool::Midpoint),
    constraint("Symmetric", Icon::CSymmetric, RelationTool::Symmetric),
    constraint("Fix", Icon::CFix, RelationTool::Fix),
];

/// Create Sketch's planes, as sketch mode names them.
const PLANES: [(OriginPlane, &str); 3] = [
    (OriginPlane::Xy, "XY plane"),
    (OriginPlane::Xz, "XZ plane"),
    (OriginPlane::Yz, "YZ plane"),
];

impl Command {
    /// Whether it's on, as `shown` has the app.
    fn enabled(&self, shown: &Shown<'_>) -> bool {
        match self.does {
            Does::Dialog(kind) => (shown.can_open)(kind),
            Does::CreateSketch => shown.create_sketch == Some(true),
            Does::Tool(_) | Does::Construction | Does::Constraint(_) => shown.in_sketch,
            Does::Filter(_) => true,
            Does::NotBuilt => false,
        }
    }

    /// Whether it stays pressed, and if so whether it is: the tool on, or Construction.
    fn pressed(&self, shown: &Shown<'_>) -> Option<(Pressed, bool)> {
        match self.does {
            Does::Tool(choice) => Some((Pressed::Tool, shown.tool == Some(choice))),
            Does::Construction => Some((Pressed::Toggle, shown.construction)),
            Does::Filter(kind) => Some((Pressed::Toggle, kind.on(shown.filter))),
            Does::Dialog(_) | Does::CreateSketch | Does::Constraint(_) | Does::NotBuilt => None,
        }
    }

    /// What its click asks, if it asks anything but to open a menu.
    fn act(&self) -> Option<Act> {
        match self.does {
            Does::Dialog(kind) => Some(Act::Dialog(kind)),
            Does::Tool(choice) => Some(Act::SketchTool(choice)),
            Does::Construction => Some(Act::Construction),
            Does::Filter(kind) => Some(Act::Filter(kind)),
            Does::Constraint(relation) => Some(Act::Relate(relation)),
            Does::CreateSketch | Does::NotBuilt => None,
        }
    }

    /// Whether the app has it.
    fn built(&self, shown: &Shown<'_>) -> bool {
        match self.does {
            Does::Dialog(_) | Does::Tool(_) | Does::Construction | Does::Constraint(_) => true,
            Does::CreateSketch => shown.create_sketch.is_some(),
            Does::Filter(_) => true,
            Does::NotBuilt => false,
        }
    }

    /// Its tooltip's line: the prototype's, and that it isn't built yet if it isn't, or,
    /// off for want of what the part has, why.
    fn said(&self, shown: &Shown<'_>) -> Option<String> {
        let why = self.why_off(shown);
        let after = if self.built(shown) {
            why
        } else {
            Some(NOT_BUILT)
        };
        match (self.description, after) {
            (description, None) => description.map(str::to_owned),
            (Some(description), Some(after)) => Some(format!("{description}. {after}")),
            (None, Some(after)) => Some(after.to_owned()),
        }
    }

    /// Why it's off, if it's a dialog's that can't open for want of what the part has.
    fn why_off(&self, shown: &Shown<'_>) -> Option<&'static str> {
        match self.does {
            Does::Dialog(kind) if !self.enabled(shown) => (shown.why_closed)(kind),
            _ => None,
        }
    }
}

// ---- The tabs (`.tb-tabs`) ----

/// The tabs' row along `rect`'s top: DESIGN, the workspace; SOLID, shown unless a sketch
/// is open; and SKETCH while one is. Clicks on a tab do nothing, as the prototype's.
fn tabs_row(ui: &mut Ui, rect: Rect, in_sketch: bool) {
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(theme::BORDER_WIDTH, theme::TOOLBAR_DIVIDER),
    );
    let row = rect.with_max_y(rect.bottom() - theme::BORDER_WIDTH);
    let builder = UiBuilder::new()
        .id_salt("toolbar_tabs")
        .max_rect(row)
        .layout(Layout::left_to_right(Align::Max));
    ui.scope_builder(builder, |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::TabList);
            node.set_label("Workspace");
        });
        ui.spacing_mut().item_spacing.x = theme::TOOLBAR_TABS_GAP;
        ui.add_space(theme::TOOLBAR_TABS_PADDING);
        workspace(ui);
        ui.add_space(theme::TOOLBAR_WORKSPACE_MARGIN);
        tab(ui, "SOLID", !in_sketch, false);
        if in_sketch {
            tab(ui, "SKETCH", true, true);
        }
    });
}

/// `text` in the toolbar's capitals, `size` points, on a `line` high line, in the colour it's
/// painted with.
fn capitals(ui: &Ui, text: &str, size: f32, line: Option<f32>) -> Arc<Galley> {
    let job = LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: FontId::proportional(size),
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            line_height: line,
            color: Color32::PLACEHOLDER,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// The workspace's name, DESIGN (`.tb-ws`): the row's height, its text at its bottom, centred
/// in its line (`--lh`) as a browser centres it, a border at its right.
fn workspace(ui: &mut Ui) {
    let galley = capitals(ui, "DESIGN", theme::HEADER_SIZE, None);
    let width = theme::TOOLBAR_WORKSPACE_PADDING_LEFT
        + galley.size().x
        + theme::TOOLBAR_WORKSPACE_PADDING_RIGHT
        + theme::BORDER_WIDTH;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, ui.available_height()), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, "DESIGN"));
    let at = Pos2::new(
        rect.left() + theme::TOOLBAR_WORKSPACE_PADDING_LEFT,
        rect.bottom()
            - theme::TOOLBAR_WORKSPACE_PADDING_BOTTOM
            - (theme::BODY_LINE_HEIGHT + galley.size().y) / 2.0,
    );
    let painter = ui.painter();
    painter.galley(at, galley, theme::TOOLBAR_WORKSPACE_TEXT);
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        Stroke::new(theme::BORDER_WIDTH, theme::TOOLBAR_WORKSPACE_BORDER),
    );
}

/// A tab (`.tb-tab`), `active` if it's the one shown, and `context` if it's SKETCH's.
fn tab(ui: &mut Ui, label: &str, active: bool, context: bool) {
    let galley = capitals(ui, label, theme::HEADER_SIZE, None);
    let size = Vec2::new(
        galley.size().x + 2.0 * theme::TOOLBAR_TAB_PADDING,
        theme::TOOLBAR_TAB_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, active, label));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Tab);
    });
    // `.tb-tab.ctx` comes after `.active`, and the shown tab keeps its look hovered.
    let (fill, text) = if context {
        (
            Some(theme::TOOLBAR_TAB_CONTEXT_BG),
            theme::TOOLBAR_TAB_CONTEXT_TEXT,
        )
    } else if active {
        (
            Some(theme::TOOLBAR_TAB_ACTIVE_BG),
            theme::TOOLBAR_TAB_ACTIVE_TEXT,
        )
    } else if response.hovered() {
        (None, theme::TOOLBAR_TAB_HOVER_TEXT)
    } else {
        (None, theme::TOOLBAR_TAB_TEXT)
    };
    let painter = ui.painter();
    if let Some(fill) = fill {
        let radius = theme::TOOLBAR_TAB_CORNER_RADIUS;
        let corners = egui::CornerRadius {
            nw: radius,
            ne: radius,
            sw: 0,
            se: 0,
        };
        painter.rect_filled(rect, corners, fill);
    }
    let at = rect.center() - galley.size() / 2.0;
    painter.galley(at, galley, text);
}

// ---- The panels (`.tb-panels`) ----

/// The panels' row in `rect`: the tab's panels from its left, and Finish Sketch at its
/// right in a sketch.
fn panels_row(ui: &mut Ui, rect: Rect, shown: &Shown<'_>, act: &mut Option<Act>) {
    let builder = UiBuilder::new()
        .id_salt("toolbar_panels")
        .max_rect(rect)
        .layout(Layout::left_to_right(Align::Min));
    ui.scope_builder(builder, |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(theme::TOOLBAR_PANELS_PADDING);
        let panels = if shown.in_sketch { SKETCH } else { SOLID };
        for (at, each) in panels.iter().enumerate() {
            // The last has no border at its right (`:last-child`), unless Finish Sketch
            // comes after it.
            let last = at + 1 == panels.len() && !shown.in_sketch;
            panel(ui, each, last, shown, act);
        }
        if shown.in_sketch && finish(ui, rect).clicked() {
            *act = Some(Act::FinishSketch);
        }
    });
}

/// A panel (`.tb-panel`): its row of buttons, its label under them, which opens its
/// drop-down, and a border at its right unless it's `last`. A toolbar for a screen reader,
/// labelled with its label.
fn panel(ui: &mut Ui, panel: &Panel, last: bool, shown: &Shown<'_>, act: &mut Option<Act>) {
    let count = panel.row.len() as f32;
    let row_width =
        count * theme::TOOLBAR_BUTTON_SIZE + (count - 1.0).max(0.0) * theme::TOOLBAR_BUTTON_GAP;
    let label = capitals(
        ui,
        panel.label,
        theme::TOOLBAR_LABEL_SIZE,
        Some(theme::TOOLBAR_LABEL_LINE_HEIGHT),
    );
    let padding = theme::TOOLBAR_LABEL_PADDING;
    let label_size = Vec2::new(
        padding.sum().x
            + label.size().x
            + theme::TOOLBAR_LABEL_GAP
            + theme::TOOLBAR_LABEL_ARROW_SIZE,
        padding.sum().y + theme::TOOLBAR_LABEL_LINE_HEIGHT,
    );
    let inside = row_width.max(label_size.x) + 2.0 * theme::TOOLBAR_PANEL_PADDING_SIDE;
    // `border-right: 1px` (`box-sizing: border-box`): a column of its own after the padding.
    let border = if last { 0.0 } else { 1.0 };
    let rect = Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(inside + border, theme::TOOLBAR_PANELS_HEIGHT),
    );
    let centre = rect.left() + inside / 2.0;
    let row = Rect::from_center_size(
        Pos2::new(
            centre,
            rect.top() + theme::TOOLBAR_PANEL_PADDING_TOP + theme::TOOLBAR_BUTTON_SIZE / 2.0,
        ),
        Vec2::new(row_width, theme::TOOLBAR_BUTTON_SIZE),
    );
    let label_rect = Rect::from_center_size(
        Pos2::new(
            centre,
            row.bottom() + theme::TOOLBAR_LABEL_MARGIN_TOP + label_size.y / 2.0,
        ),
        label_size,
    );
    ui.scope_builder(UiBuilder::new().id_salt(panel.label).max_rect(rect), |ui| {
        ui.set_min_size(rect.size());
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::Toolbar);
            node.set_label(panel.label);
        });
        let buttons = UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center));
        ui.scope_builder(buttons, |ui| {
            ui.spacing_mut().item_spacing.x = theme::TOOLBAR_BUTTON_GAP;
            for command in panel.row {
                row_button(ui, command, shown, act);
            }
        });
        let opener = ui.put(label_rect, |ui: &mut Ui| {
            drop_down_label(ui, panel.label, label)
        });
        bar::menu(&opener, |ui| {
            crate::menu::node(ui, panel.label, |ui| match panel.menu {
                Some(entries) => {
                    for entry in entries {
                        menu_entry(ui, entry, shown, act);
                    }
                }
                None => {
                    for command in panel.row {
                        menu_entry(ui, &Entry::Command(*command), shown, act);
                    }
                }
            });
        });
    });
    if border > 0.0 {
        ui.painter().vline(
            rect.right() - 0.5,
            rect.y_range(),
            Stroke::new(theme::BORDER_WIDTH, theme::TOOLBAR_DIVIDER),
        );
    }
}

/// A command's button in a panel's row, with its tooltip, its keys named if the registry
/// gives it any; a click opens its dialog, or, for Create Sketch, its planes; or chooses its
/// tool, or turns Construction on or off.
fn row_button(ui: &mut Ui, command: &Command, shown: &Shown<'_>, act: &mut Option<Act>) {
    let response = tool_button(
        ui,
        command.icon,
        command.label,
        command.enabled(shown),
        command.pressed(shown),
    );
    let said = command.said(shown);
    let keys = match command.does {
        Does::Filter(kind) => Some(filter_command(kind)),
        _ => command.key,
    }
    .and_then(|key| key.shown(ui.ctx()));
    let response = bar::tooltip(response, command.label, keys, said.as_deref());
    if command.does == Does::CreateSketch {
        bar::menu(&response, |ui| {
            if let Some(plane) = planes(ui) {
                *act = Some(Act::CreateSketch(plane));
            }
        });
    } else if response.clicked()
        && let Some(asked) = command.act()
    {
        *act = Some(asked);
    }
}

/// How a button that stays pressed shows it is: a tool on (`.tb-btn.on`), or a toggle on
/// (`.tb-btn.toggle-on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pressed {
    Tool,
    Toggle,
}

/// A toolbar button (`.tb-btn`): `icon`, labelled `label`, off unless `enabled`; and, if
/// it stays pressed, which way and whether it is, shown so to a screen reader too.
fn tool_button(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    pressed: Option<(Pressed, bool)>,
) -> Response {
    // Painted at the toolbar's opacity, not faded further when off: `.tb-btn:disabled` is
    // Gray.400 itself.
    let opacity = ui.opacity();
    let own = enabled;
    ui.add_enabled(enabled, |ui: &mut Ui| {
        ui.set_opacity(opacity);
        let size = Vec2::splat(theme::TOOLBAR_BUTTON_SIZE);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| match pressed {
            Some((_, on)) => WidgetInfo::selected(WidgetType::Button, enabled, on, label),
            None => WidgetInfo::labeled(WidgetType::Button, enabled, label),
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let corners = theme::TOOLBAR_BUTTON_CORNER_RADIUS;
        // `.tb-btn:hover:not(:disabled)` outweighs `.on`'s and `.toggle-on`'s background and
        // colour, and leaves `.toggle-on`'s box-shadow as it is. Behind a modal, drawn as
        // its own flag has it (#487).
        let on = crate::modal::looks_on(ui, own);
        let (fill, color) = match (on, response.hovered(), pressed) {
            (false, _, _) => (None, theme::TOOLBAR_BUTTON_DISABLED_ICON),
            (true, true, _) => (
                Some(theme::TOOLBAR_BUTTON_HOVER_BG),
                theme::TOOLBAR_BUTTON_HOVER_ICON,
            ),
            (true, false, Some((Pressed::Tool, true))) => (
                Some(theme::TOOLBAR_BUTTON_ON_BG),
                theme::TOOLBAR_BUTTON_ON_ICON,
            ),
            (true, false, Some((Pressed::Toggle, true))) => (
                Some(theme::TOOLBAR_BUTTON_TOGGLED_BG),
                theme::TOOLBAR_BUTTON_TOGGLED_ICON,
            ),
            (true, false, Some((_, false)) | None) => (None, theme::TOOLBAR_BUTTON_ICON),
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, corners, fill);
        }
        if pressed == Some((Pressed::Toggle, true)) {
            toggled_mark(ui, rect);
        }
        theme::focus_ring(
            ui,
            &response,
            rect,
            theme::TOOLBAR_BUTTON_CORNER_RADIUS,
            StrokeKind::Inside,
        );
        icon.image(IconSize::Toolbar).tint(color).paint_at(
            ui,
            Rect::from_center_size(rect.center(), Vec2::splat(TOOLBAR_ICON_SIZE)),
        );
        response
    })
}

/// A toggle button's mark while it's on (`.tb-btn.toggle-on`'s `box-shadow: inset 0 -2px 0
/// var(--blue)`), hovered or not: a blue line along its foot.
fn toggled_mark(ui: &Ui, rect: Rect) {
    let foot = rect.bottom() - theme::TOOLBAR_BUTTON_TOGGLED_MARK_WIDTH / 2.0;
    ui.painter().hline(
        rect.x_range(),
        foot,
        Stroke::new(
            theme::TOOLBAR_BUTTON_TOGGLED_MARK_WIDTH,
            theme::TOOLBAR_BUTTON_TOGGLED_MARK,
        ),
    );
}

/// A panel's label (`.tb-label`), its text `galley` and the arrow after it, labelled
/// `label`; a click opens its drop-down.
fn drop_down_label(ui: &mut Ui, label: &str, galley: Arc<Galley>) -> Response {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    let color = if response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::TOOLBAR_LABEL_CORNER_RADIUS,
            theme::TOOLBAR_LABEL_HOVER_BG,
        );
        theme::TOOLBAR_LABEL_HOVER_TEXT
    } else {
        theme::TOOLBAR_LABEL_TEXT
    };
    theme::focus_ring(
        ui,
        &response,
        rect,
        theme::TOOLBAR_LABEL_CORNER_RADIUS,
        StrokeKind::Inside,
    );
    let left = rect.left() + f32::from(theme::TOOLBAR_LABEL_PADDING.left);
    let text = Pos2::new(left, rect.center().y - galley.size().y / 2.0);
    let arrow = Rect::from_min_size(
        Pos2::new(
            text.x + galley.size().x + theme::TOOLBAR_LABEL_GAP,
            rect.center().y - theme::TOOLBAR_LABEL_ARROW_SIZE / 2.0,
        ),
        Vec2::splat(theme::TOOLBAR_LABEL_ARROW_SIZE),
    );
    ui.painter().galley(text, galley, color);
    Icon::Down
        .image(IconSize::Small)
        .fit_to_exact_size(arrow.size())
        .tint(color)
        .paint_at(ui, arrow);
    response
}

/// One of a drop-down's entries (`.menu .mi`, `.mh`, `.msep`).
fn menu_entry(ui: &mut Ui, entry: &Entry, shown: &Shown<'_>, act: &mut Option<Act>) {
    match entry {
        Entry::Separator => {
            ui.separator();
        }
        Entry::Heading(text) => {
            ui.label(
                RichText::new(*text)
                    .size(theme::TOOLBAR_LABEL_SIZE)
                    .color(theme::TOOLBAR_LABEL_TEXT)
                    .extra_letter_spacing(theme::CAPITALS_LETTER_SPACING),
            );
        }
        Entry::AllFilters => {
            if ui.button("Select all filters").clicked() {
                *act = Some(Act::AllFilters);
                ui.close();
            }
        }
        Entry::ClearSelection => {
            let command = crate::shortcuts::Command::ClearSelection;
            let clear =
                egui::Button::new(command.label()).shortcut_text(command.keys_text(ui.ctx()));
            if ui.add_enabled(shown.selected, clear).clicked() {
                *act = Some(Act::ClearSelection);
                ui.close();
            }
        }
        Entry::Command(command) => {
            let item = egui::Button::image_and_text(icons::icon(command.icon), command.label);
            let enabled = command.enabled(shown);
            match command.does {
                Does::Tool(_) | Does::Construction => {
                    let on = command.pressed(shown).is_some_and(|(_, on)| on);
                    if ui.add_enabled(enabled, item.selected(on)).clicked() {
                        *act = command.act();
                        ui.close();
                    }
                }
                Does::CreateSketch => {
                    let (button, _) = ui
                        .add_enabled_ui(enabled, |ui| {
                            egui::containers::menu::SubMenuButton::from_button(item).ui(ui, |ui| {
                                if let Some(plane) = planes(ui) {
                                    *act = Some(Act::CreateSketch(plane));
                                }
                            })
                        })
                        .inner;
                    if let (false, Some(said)) = (command.built(shown), command.said(shown)) {
                        button.on_disabled_hover_text(said);
                    }
                }
                Does::Dialog(_) | Does::Constraint(_) => {
                    let item = ui.add_enabled(enabled, item);
                    let item = match command.why_off(shown) {
                        Some(why) => item.on_disabled_hover_text(why),
                        None => item,
                    };
                    if item.clicked() {
                        *act = command.act();
                        ui.close();
                    }
                }
                Does::Filter(kind) => {
                    // Checked while its kind is taken (the prototype's `✓`), its key beside it.
                    let key = filter_command(kind).keys_text(ui.ctx());
                    let item = item.selected(kind.on(shown.filter)).shortcut_text(key);
                    if ui.add(item).clicked() {
                        *act = Some(Act::Filter(kind));
                        ui.close();
                    }
                }
                Does::NotBuilt => {
                    let item = ui.add_enabled(false, item);
                    if let Some(said) = command.said(shown) {
                        item.on_disabled_hover_text(said);
                    }
                }
            }
        }
    }
}

/// The registry's command for a filter's kind, whose keys its menu item names.
fn filter_command(kind: FilterKind) -> crate::shortcuts::Command {
    match kind {
        FilterKind::Faces => crate::shortcuts::Command::FilterFace,
        FilterKind::Edges => crate::shortcuts::Command::FilterEdge,
        FilterKind::Vertices => crate::shortcuts::Command::FilterVertex,
    }
}

/// Create Sketch's planes, a menu of their own for a screen reader, labelled Create Sketch:
/// a click on one closes the menu, and is the plane a sketch is started on. The one menu of
/// them, which Create Sketch's button opens, and its item in CREATE's drop-down, and the
/// canvas's context menu over nothing (`crate::canvas_menu`).
///
/// While they're drawn, the viewport draws all three origin planes, the one whose item the
/// pointer is on (or has the keys' focus) lit (`crate::planes`, the prototype's
/// `startPickPlane`).
pub(crate) fn planes(ui: &mut Ui) -> Option<OriginPlane> {
    let mut chosen = None;
    let mut lit = None;
    crate::menu::node(ui, CREATE_SKETCH.label, |ui| {
        for (plane, word) in PLANES {
            let item = ui.button(word);
            if item.hovered() || item.has_focus() {
                lit = Some(plane);
            }
            if item.clicked() {
                chosen = Some(plane);
                ui.close();
            }
        }
    });
    crate::planes::menu_drawn(ui.ctx(), lit);
    chosen
}

// ---- Finish Sketch (`.tb-finish`) ----

/// Finish Sketch (`.btn-finish`), at the right of the panels' `row`, its height centred in
/// it, with its tooltip.
fn finish(ui: &mut Ui, row: Rect) -> Response {
    let label = "Finish Sketch";
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(theme::BODY_SIZE),
        theme::TOOLBAR_FINISH_TEXT,
    );
    let width = 2.0 * theme::TOOLBAR_FINISH_PADDING
        + ICON_SIZE
        + theme::TOOLBAR_FINISH_GAP
        + galley.size().x;
    let right = row.right() - theme::TOOLBAR_PANELS_PADDING - theme::TOOLBAR_FINISH_MARGIN;
    let rect = Rect::from_min_max(
        Pos2::new(
            right - width,
            row.center().y - theme::TOOLBAR_BUTTON_SIZE / 2.0,
        ),
        Pos2::new(right, row.center().y + theme::TOOLBAR_BUTTON_SIZE / 2.0),
    );
    let response = ui.interact(rect, ui.id().with("finish_sketch"), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    let fill = if response.hovered() {
        theme::TOOLBAR_FINISH_HOVER_BG
    } else {
        theme::TOOLBAR_FINISH_BG
    };
    let painter = ui.painter();
    painter.rect_filled(rect, theme::SMALL_CORNER_RADIUS, fill);
    theme::focus_ring(
        ui,
        &response,
        rect,
        theme::SMALL_CORNER_RADIUS,
        StrokeKind::Inside,
    );
    let icon = Rect::from_min_size(
        Pos2::new(
            rect.left() + theme::TOOLBAR_FINISH_PADDING,
            rect.center().y - ICON_SIZE / 2.0,
        ),
        Vec2::splat(ICON_SIZE),
    );
    Icon::Check
        .image(IconSize::Small)
        .tint(theme::TOOLBAR_FINISH_TEXT)
        .paint_at(ui, icon);
    let at = Pos2::new(
        icon.right() + theme::TOOLBAR_FINISH_GAP,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(at, galley, theme::TOOLBAR_FINISH_TEXT);
    let finish = shortcuts::Command::FinishSketch;
    bar::tooltip(
        response,
        label,
        finish.shown(ui.ctx()),
        finish.entry().description,
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use eframe::egui::accesskit::Toggled;
    use eframe::egui::epaint::{ClippedShape, RectShape, TextShape};
    use eframe::egui::{CornerRadius, Shape, TextFormat};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

    #[test]
    fn a_tool_button_behind_a_modal_keeps_its_own_look_on_or_off_and_pressed() {
        for pressed in [
            None,
            Some((Pressed::Tool, true)),
            Some((Pressed::Toggle, true)),
        ] {
            crate::modal::tests::drawn_alike_behind(|ui, on| {
                tool_button(ui, Icon::Pad, "Pad", on, pressed);
            });
        }
    }

    /// A dialog's command is labelled with its kind's title, which the dialog's heading
    /// and the tests find it by; each sweep is in CREATE's drop-down once and in its row
    /// once (app.js's `renderToolbar`, interact.js's `createMenu`); the dressings and the
    /// booleans are in MODIFY's row and drop-down once each (`renderToolbar`, `modifyMenu`).
    #[test]
    fn each_sweep_and_modify_dialog_is_offered_once_under_its_title() {
        let create = &SOLID[0];
        let menu = create.menu.unwrap_or_default();
        let modify = &SOLID[1];
        assert_eq!(modify.label, "MODIFY");
        for kind in Kind::ALL.into_iter().filter(|kind| kind.modifies()) {
            let commands = |entries: Vec<&Command>| {
                entries
                    .into_iter()
                    .filter(|command| command.does == Does::Dialog(kind))
                    .map(|command| command.label)
                    .collect::<Vec<_>>()
            };
            let in_menu = commands(
                modify
                    .menu
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|entry| match entry {
                        Entry::Command(command) => Some(command),
                        _ => None,
                    })
                    .collect(),
            );
            assert_eq!(in_menu, [kind.title()], "{kind:?}");
            assert_eq!(
                commands(modify.row.iter().collect()),
                [kind.title()],
                "{kind:?}"
            );
        }
        for kind in Kind::ALL.into_iter().filter(|kind| kind.sweeps()) {
            let in_menu: Vec<_> = menu
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Command(command) if command.does == Does::Dialog(kind) => Some(command),
                    _ => None,
                })
                .collect();
            assert_eq!(in_menu.len(), 1, "{kind:?}");
            assert_eq!(in_menu[0].label, kind.title());
            let in_row = create
                .row
                .iter()
                .filter(|command| command.does == Does::Dialog(kind))
                .count();
            assert_eq!(in_row, 1, "{kind:?}");
        }
    }

    /// Solids are made sketch-first, as SolidWorks makes them (Trevin's decision of
    /// 2026-11-09): no panel of SOLID's or SKETCH's, in its row or its drop-down, makes a
    /// box, a cylinder, a sphere, a cone, or a torus, and none is labelled as one; the
    /// primitives' dialogs open only to edit a feature a file holds. Every kind but these
    /// five is offered somewhere, so the check can't pass by offering nothing.
    #[test]
    fn no_panel_makes_a_primitive() {
        let primitives = [
            Kind::Box,
            Kind::Cylinder,
            Kind::Sphere,
            Kind::Cone,
            Kind::Torus,
        ];
        let titles = ["Box", "Cylinder", "Sphere", "Cone", "Torus"];
        let mut offered: Vec<&Command> = Vec::new();
        for panel in SOLID.iter().chain(SKETCH) {
            offered.extend(panel.row);
            for entry in panel.menu.unwrap_or_default() {
                match entry {
                    Entry::Command(command) => offered.push(command),
                    Entry::Heading(text) => {
                        assert_ne!(*text, "PRIMITIVES", "{}", panel.label);
                    }
                    Entry::Separator | Entry::AllFilters | Entry::ClearSelection => {}
                }
            }
        }
        for command in &offered {
            if let Does::Dialog(kind) = command.does {
                assert!(!primitives.contains(&kind), "{}: {kind:?}", command.label);
            }
            assert!(!titles.contains(&command.label), "{}", command.label);
        }
        for kind in Kind::ALL
            .into_iter()
            .filter(|kind| !primitives.contains(kind))
        {
            assert!(
                offered
                    .iter()
                    .any(|command| command.does == Does::Dialog(kind)),
                "{kind:?} offered nowhere"
            );
        }
    }

    // ---- The toolbar as drawn, against the prototype ----
    //
    // Each value below is typed from the approved prototype's stylesheet
    // (docs/design/m5-ui/styles.css: `:root`'s tokens, `svg.i`, `#toolbar`, `.tb-*`, and
    // `.btn-finish`) and js/app.js's renderToolbar, never from theme.rs, and compared with
    // what the toolbar painted: each rectangle, line, icon, and text from egui's shapes, and
    // each widget's bounds from the accessibility tree.

    /// The prototype's tokens and geometry.
    mod css {
        use eframe::egui::Color32;

        // `:root`'s greys and colours.
        pub const G100: Color32 = Color32::from_rgb(0x0d, 0x0d, 0x0d);
        pub const G200: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
        pub const G400: Color32 = Color32::from_rgb(0x64, 0x63, 0x63);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        pub const BLUE: Color32 = Color32::from_rgb(0x18, 0x6a, 0xdd);
        pub const BLUE_TEXT: Color32 = Color32::from_rgb(0xf0, 0xf2, 0xff);
        /// `.btn-finish:hover`'s `background: #2a78e8`.
        pub const FINISH_HOVER: Color32 = Color32::from_rgb(0x2a, 0x78, 0xe8);

        /// `--fs`, `--lh`, `--fs-small`, `--icon` (`svg.i`), `svg.i.lg`, `--r-sm`.
        pub const FS: f32 = 12.0;
        pub const LH: f32 = 16.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        pub const ICON_LG: f32 = 20.0;
        pub const R_SM: u8 = 4;
        /// `--toolbar-tabs-h`, `--toolbar-h`; `#toolbar`'s and `.tb-tabs`' `border-bottom:
        /// 1px` (`border-box`, `*`'s).
        pub const TABS_H: f32 = 24.0;
        pub const PANELS_H: f32 = 52.0;
        pub const BORDER: f32 = 1.0;
        /// `.tb-tabs`: `padding-left: 8px; gap: 2px`.
        pub const TABS_PADDING: f32 = 8.0;
        pub const TABS_GAP: f32 = 2.0;
        /// `.tb-ws`: `padding: 0 10px 5px 4px; margin-right: 6px`; its and `.tb-tab`'s and
        /// `.tb-label`'s `letter-spacing: .5px`.
        pub const WS_PADDING_LEFT: f32 = 4.0;
        pub const WS_PADDING_RIGHT: f32 = 10.0;
        pub const WS_PADDING_BOTTOM: f32 = 5.0;
        pub const WS_MARGIN: f32 = 6.0;
        pub const CAPITALS_SPACING: f32 = 0.5;
        /// `.tb-tab`: `padding: 0 10px; height: 22px`.
        pub const TAB_PADDING: f32 = 10.0;
        pub const TAB_H: f32 = 22.0;
        /// `.tb-panels`: `padding: 0 6px`; `.tb-panel`: `padding: 4px 6px 0`.
        pub const PANELS_PADDING: f32 = 6.0;
        pub const PANEL_PADDING_TOP: f32 = 4.0;
        pub const PANEL_PADDING_SIDE: f32 = 6.0;
        /// `.tb-row { gap: 1px }`; `.tb-btn`: 30 square.
        pub const ROW_GAP: f32 = 1.0;
        pub const BTN: f32 = 30.0;
        /// `.tb-label`: `font-size: 10px; line-height: 14px; gap: 2px; padding: 1px 4px;
        /// border-radius: 3px; margin-top: 1px`, its arrow (`svg`) 8 square.
        pub const LABEL_FS: f32 = 10.0;
        pub const LABEL_LH: f32 = 14.0;
        pub const LABEL_GAP: f32 = 2.0;
        pub const LABEL_PADDING_Y: f32 = 1.0;
        pub const LABEL_PADDING_X: f32 = 4.0;
        pub const LABEL_RADIUS: u8 = 3;
        pub const LABEL_MARGIN: f32 = 1.0;
        pub const LABEL_ARROW: f32 = 8.0;
        /// `.tb-finish`: `padding: 0 8px`; `.btn-finish`: `height: 30px; padding: 0 12px;
        /// gap: 6px`.
        pub const FINISH_MARGIN: f32 = 8.0;
        pub const FINISH_H: f32 = 30.0;
        pub const FINISH_PADDING: f32 = 12.0;
        pub const FINISH_GAP: f32 = 6.0;
    }

    /// The window's width.
    const WIDTH: f32 = 1280.0;

    /// The toolbar across a window, in the app's theme, a sketch open if the state says so;
    /// Pocket's dialog the one that can't open, and Create Sketch on.
    fn drawn(in_sketch: bool) -> Harness<'static, bool> {
        drawn_with(in_sketch, &Sketching::default())
    }

    /// In a sketch, the tool on and whether Construction is, which a test changes between
    /// frames, and what the toolbar was last asked.
    #[derive(Default)]
    struct Sketching {
        tool: Rc<Cell<Option<Choice>>>,
        construction: Rc<Cell<bool>>,
        asked: Rc<Cell<Option<Act>>>,
    }

    #[test]
    fn a_dialogs_command_says_why_its_off_only_while_its_off() {
        // `why_closed` read only for a command that's off: on, Union's tooltip is its own
        // words (it has none), whatever `why_closed` would say; off, the reason. A stub
        // that always gives a reason tells the two apart.
        let why = |_: Kind| Some("Needs two bodies");
        let on = |_: Kind| true;
        let off = |_: Kind| false;
        let mut shown = Shown {
            in_sketch: false,
            create_sketch: Some(true),
            can_open: &on,
            why_closed: &why,
            selected: false,
            tool: None,
            construction: false,
            filter: Filter::default(),
        };
        assert_eq!(UNION.said(&shown), None);
        shown.can_open = &off;
        assert_eq!(UNION.said(&shown).as_deref(), Some("Needs two bodies"));
    }

    fn drawn_with(in_sketch: bool, sketching: &Sketching) -> Harness<'static, bool> {
        let (tool, construction, asked) = (
            sketching.tool.clone(),
            sketching.construction.clone(),
            sketching.asked.clone(),
        );
        let mut h = Harness::builder()
            .with_size(Vec2::new(WIDTH, 200.0))
            .build_ui_state(
                move |ui, in_sketch: &mut bool| {
                    let can_open = |kind: Kind| kind != Kind::Pocket;
                    let shown = Shown {
                        in_sketch: *in_sketch,
                        create_sketch: Some(true),
                        can_open: &can_open,
                        why_closed: &|_| None,
                        selected: false,
                        tool: tool.get(),
                        construction: construction.get(),
                        filter: Filter::default(),
                    };
                    let window = UiBuilder::new().max_rect(ui.ctx().content_rect());
                    if let Some(act) = ui.scope_builder(window, |ui| show(ui, &shown)).inner {
                        asked.set(Some(act));
                    }
                },
                in_sketch,
            );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// Every shape the last frame painted, nested ones flattened, each with its clip.
    fn clipped(h: &Harness<'_, bool>) -> Vec<ClippedShape> {
        fn flatten(clip: Rect, shape: &Shape, out: &mut Vec<ClippedShape>) {
            match shape {
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(clip, shape, out)),
                shape => out.push(ClippedShape {
                    clip_rect: clip,
                    shape: shape.clone(),
                }),
            }
        }
        let mut out = Vec::new();
        for shown in &h.output().shapes {
            flatten(shown.clip_rect, &shown.shape, &mut out);
        }
        out
    }

    fn rects(h: &Harness<'_, bool>) -> Vec<RectShape> {
        clipped(h)
            .into_iter()
            .filter_map(|shown| match shown.shape {
                Shape::Rect(rect) => Some(rect),
                _ => None,
            })
            .collect()
    }

    /// The one text painted whose text is `text`.
    #[track_caller]
    fn text(h: &Harness<'_, bool>, text: &str) -> TextShape {
        let mut found: Vec<TextShape> = clipped(h)
            .into_iter()
            .filter_map(|shown| match shown.shape {
                Shape::Text(shown) if shown.galley.text() == text => Some(shown),
                _ => None,
            })
            .collect();
        assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
        found.remove(0)
    }

    /// A text's colour as painted: its override, else its first section's, else the
    /// shape's own for a placeholder.
    fn colour(shown: &TextShape) -> Color32 {
        let format = shown.galley.job.sections[0].format.color;
        shown
            .override_text_color
            .unwrap_or(if format == Color32::PLACEHOLDER {
                shown.fallback_color
            } else {
                format
            })
    }

    fn format(shown: &TextShape) -> &TextFormat {
        &shown.galley.job.sections[0].format
    }

    /// The toolbar's capitals: `size` points, `.5px` apart, on a `line` high line.
    #[track_caller]
    fn capitals(shown: &TextShape, size: f32, line: Option<f32>) {
        let format = format(shown);
        assert_eq!(format.font_id, FontId::proportional(size));
        assert_eq!(format.extra_letter_spacing, css::CAPITALS_SPACING);
        assert_eq!(format.line_height, line);
    }

    /// The tint of the icon painted at `rect` (a rectangle filled with its texture), to
    /// half a point: egui puts an image on the pixel grid.
    fn icon_at(h: &Harness<'_, bool>, rect: Rect) -> Option<Color32> {
        rects(h)
            .into_iter()
            .find(|shape| {
                shape.brush.is_some()
                    && (shape.rect.min - rect.min).abs().max_elem() <= 0.5
                    && (shape.rect.max - rect.max).abs().max_elem() <= 0.5
            })
            .map(|shape| shape.fill)
    }

    /// The fill painted over the whole of `rect`, if any but a transparent one is, and its
    /// corners.
    fn fill_at(h: &Harness<'_, bool>, rect: Rect) -> Option<(Color32, CornerRadius)> {
        rects(h)
            .into_iter()
            .find(|shape| {
                shape.brush.is_none()
                    && shape.fill != Color32::TRANSPARENT
                    && near_rect(shape.rect, rect)
            })
            .map(|shape| (shape.fill, shape.corner_radius))
    }

    /// The lines painted from `from` to `to`, by their strokes.
    fn lines(h: &Harness<'_, bool>, from: Pos2, to: Pos2) -> Vec<Stroke> {
        clipped(h)
            .into_iter()
            .filter_map(|shown| match shown.shape {
                Shape::LineSegment { points, stroke }
                    if near_pos(points[0], from) && near_pos(points[1], to) =>
                {
                    Some(stroke)
                }
                _ => None,
            })
            .collect()
    }

    /// The vertical lines painted right of `x`.
    fn lines_right_of(h: &Harness<'_, bool>, x: f32) -> usize {
        clipped(h)
            .into_iter()
            .filter(|shown| {
                matches!(shown.shape, Shape::LineSegment { points, .. }
                    if near(points[0].x, points[1].x) && points[0].x > x)
            })
            .count()
    }

    /// Within egui's rounding of a widget's place, a 32nd of a point.
    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= egui::emath::GUI_ROUNDING
    }

    fn near_pos(a: Pos2, b: Pos2) -> bool {
        near(a.x, b.x) && near(a.y, b.y)
    }

    fn near_rect(a: Rect, b: Rect) -> bool {
        near_pos(a.min, b.min) && near_pos(a.max, b.max)
    }

    #[track_caller]
    fn at(got: Pos2, want: Pos2, what: &str) {
        assert!(near_pos(got, want), "{what}: {got:?}, not {want:?}");
    }

    #[track_caller]
    fn sized(got: Rect, want: Rect, what: &str) {
        assert!(
            near_rect(got, want),
            "{what}: {:?}, not {:?}",
            [got.min.x, got.min.y, got.max.x, got.max.y],
            [want.min.x, want.min.y, want.max.x, want.max.y]
        );
    }

    fn widget(h: &Harness<'_, bool>, role: Role, label: &str) -> Rect {
        h.get_by_role_and_label(role, label).rect()
    }

    fn hover(h: &mut Harness<'_, bool>, role: Role, label: &str) {
        h.get_by_role_and_label(role, label).hover();
        h.run_steps(2);
    }

    /// A panel as the prototype lays it out: its box, its border, its row of buttons, and
    /// its label.
    struct Laid {
        name: &'static str,
        rect: Rect,
        border: bool,
        buttons: Vec<(&'static str, Rect)>,
        label: Rect,
    }

    /// `panels`, each its name and its buttons, laid out from `.tb-panels`' left as
    /// `.tb-panel`, `.tb-row`, and `.tb-label` have them, the last with no border unless
    /// `finish` comes after it.
    fn laid(
        h: &Harness<'_, bool>,
        panels: &[(&'static str, &[&'static str])],
        finish: bool,
    ) -> Vec<Laid> {
        let mut left = css::PANELS_PADDING;
        let mut out = Vec::new();
        for (at, (name, buttons)) in panels.iter().enumerate() {
            let label_text = text(h, name).galley.size().x;
            let label = Vec2::new(
                css::LABEL_PADDING_X
                    + label_text
                    + css::LABEL_GAP
                    + css::LABEL_ARROW
                    + css::LABEL_PADDING_X,
                css::LABEL_PADDING_Y + css::LABEL_LH + css::LABEL_PADDING_Y,
            );
            let count = buttons.len() as f32;
            let row = count * css::BTN + (count - 1.0) * css::ROW_GAP;
            let inside = css::PANEL_PADDING_SIDE + row.max(label.x) + css::PANEL_PADDING_SIDE;
            let border = finish || at + 1 < panels.len();
            let rect = Rect::from_min_size(
                Pos2::new(left, css::TABS_H),
                Vec2::new(
                    inside + if border { css::BORDER } else { 0.0 },
                    css::PANELS_H,
                ),
            );
            let middle = left + inside / 2.0;
            let top = css::TABS_H + css::PANEL_PADDING_TOP;
            let buttons = buttons
                .iter()
                .enumerate()
                .map(|(k, button)| {
                    let x = middle - row / 2.0 + k as f32 * (css::BTN + css::ROW_GAP);
                    (
                        *button,
                        Rect::from_min_size(Pos2::new(x, top), Vec2::splat(css::BTN)),
                    )
                })
                .collect();
            let label = Rect::from_min_size(
                Pos2::new(middle - label.x / 2.0, top + css::BTN + css::LABEL_MARGIN),
                label,
            );
            out.push(Laid {
                name,
                rect,
                border,
                buttons,
                label,
            });
            left = rect.right();
        }
        out
    }

    /// SOLID's panels and their buttons (app.js's renderToolbar).
    const SOLID_PANELS: [(&str, &[&str]); 4] = [
        (
            "CREATE",
            &["Create Sketch", "Pad", "Pocket", "Revolve", "Groove"],
        ),
        (
            "MODIFY",
            &["Fillet", "Chamfer", "Union", "Subtract", "Intersect"],
        ),
        ("INSPECT", &["Measure"]),
        ("SELECT", &["Faces", "Edges", "Vertices"]),
    ];

    /// SKETCH's.
    const SKETCH_PANELS: [(&str, &[&str]); 4] = [
        (
            "CREATE",
            &[
                "Line",
                "Rectangle",
                "Circle",
                "Arc",
                "Point",
                "Construction",
            ],
        ),
        ("MODIFY", &["Trim", "Extend", "Split"]),
        (
            "CONSTRAINTS",
            &[
                "Smart Dimension",
                "Horizontal",
                "Vertical",
                "Coincident",
                "Tangent",
                "Equal",
                "Parallel",
                "Perpendicular",
                "Midpoint",
                "Symmetric",
                "Fix",
            ],
        ),
        ("INSPECT", &["Measure"]),
    ];

    /// Each panel at its place, labelled with its label, with a border of Gray.200 down its
    /// right unless it has none, and none past it.
    #[track_caller]
    fn panels_placed(h: &Harness<'_, bool>, laid: &[Laid]) {
        for panel in laid {
            let x = panel.rect.right() - css::BORDER / 2.0;
            let border = lines(
                h,
                Pos2::new(x, panel.rect.top()),
                Pos2::new(x, panel.rect.bottom()),
            );
            if panel.border {
                assert_eq!(
                    border,
                    [Stroke::new(css::BORDER, css::G200)],
                    "{}",
                    panel.name
                );
            } else {
                assert_eq!(
                    lines_right_of(h, panel.rect.left()),
                    0,
                    "{}'s border",
                    panel.name
                );
            }
        }
    }

    #[test]
    fn the_tabs_are_drawn_as_tb_tabs_tb_ws_and_tb_tab() {
        let mut h = drawn(false);
        // `#toolbar`: Gray.100, the tabs' 24 and the panels' 52 over its border, 1 of
        // Gray.250 along its last point; `.tb-tabs`' border, 1 of Gray.200 along theirs.
        let height = css::TABS_H + css::PANELS_H + css::BORDER;
        assert_eq!(
            fill_at(
                &h,
                Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, height))
            ),
            Some((css::G100, CornerRadius::ZERO))
        );
        for (y, ink) in [
            (height - css::BORDER / 2.0, css::G250),
            (css::TABS_H - css::BORDER / 2.0, css::G200),
        ] {
            assert_eq!(
                lines(&h, Pos2::new(0.0, y), Pos2::new(WIDTH, y)),
                [Stroke::new(css::BORDER, ink)],
                "the border at {y}"
            );
        }
        // `.tb-ws`: 8 in, the row's height above the border; 4, DESIGN, 10, and a border of
        // Gray.250 at its right; its text Gray.550 on a 16 line (`--lh`) 5 above its bottom.
        let row = css::TABS_H - css::BORDER;
        let design = text(&h, "DESIGN");
        capitals(&design, css::FS_SMALL, None);
        assert_eq!(colour(&design), css::G550);
        let ws = Rect::from_min_max(
            Pos2::new(css::TABS_PADDING, 0.0),
            Pos2::new(
                css::TABS_PADDING
                    + css::WS_PADDING_LEFT
                    + design.galley.size().x
                    + css::WS_PADDING_RIGHT
                    + css::BORDER,
                row,
            ),
        );
        sized(widget(&h, Role::Label, "DESIGN"), ws, "DESIGN");
        at(
            design.pos,
            Pos2::new(
                ws.left() + css::WS_PADDING_LEFT,
                row - css::WS_PADDING_BOTTOM - css::LH / 2.0 - design.galley.size().y / 2.0,
            ),
            "DESIGN",
        );
        let x = ws.right() - css::BORDER / 2.0;
        assert_eq!(
            lines(&h, Pos2::new(x, 0.0), Pos2::new(x, row)),
            [Stroke::new(css::BORDER, css::G250)]
        );
        // `.tb-tab`, the gap and the workspace's margin on: 10, its name, and 10; 22 high at
        // the row's bottom; its name centred.
        let tab = |h: &Harness<'_, bool>, label: &str, left: f32| {
            let shown = text(h, label);
            capitals(&shown, css::FS_SMALL, None);
            let rect = Rect::from_min_max(
                Pos2::new(left, row - css::TAB_H),
                Pos2::new(
                    left + css::TAB_PADDING + shown.galley.size().x + css::TAB_PADDING,
                    row,
                ),
            );
            sized(widget(h, Role::Tab, label), rect, label);
            at(shown.pos, rect.center() - shown.galley.size() / 2.0, label);
            (rect, colour(&shown))
        };
        let top = CornerRadius {
            nw: css::R_SM,
            ne: css::R_SM,
            sw: 0,
            se: 0,
        };
        // SOLID shown (`.active`): Gray.200 behind white, its top corners `--r-sm`.
        let left = ws.right() + css::TABS_GAP + css::WS_MARGIN;
        let (solid, ink) = tab(&h, "SOLID", left);
        assert_eq!(fill_at(&h, solid), Some((css::G200, top)));
        assert_eq!(ink, css::G1000);
        // A sketch open: SOLID Gray.550 on nothing (`.tb-tab`), Gray.800 hovered; SKETCH
        // after it, `.ctx.active`: blue behind Blue.900, hovered or not.
        *h.state_mut() = true;
        h.run_steps(4);
        let (solid, ink) = tab(&h, "SOLID", left);
        assert_eq!(fill_at(&h, solid), None);
        assert_eq!(ink, css::G550);
        let (sketch, ink) = tab(&h, "SKETCH", solid.right() + css::TABS_GAP);
        assert_eq!(fill_at(&h, sketch), Some((css::BLUE, top)));
        assert_eq!(ink, css::BLUE_TEXT);
        hover(&mut h, Role::Tab, "SOLID");
        assert_eq!(tab(&h, "SOLID", left).1, css::G800);
        assert_eq!(fill_at(&h, solid), None);
        hover(&mut h, Role::Tab, "SKETCH");
        assert_eq!(tab(&h, "SKETCH", sketch.left()).1, css::BLUE_TEXT);
        assert_eq!(fill_at(&h, sketch), Some((css::BLUE, top)));
    }

    #[test]
    fn the_panels_and_their_buttons_are_drawn_as_tb_panel_and_tb_btn() {
        let mut h = drawn(false);
        // `.tb-panels`' 6 in, each `.tb-panel` its row or its label, whichever is wider, in
        // its 6 at each side, and its border; SELECT, the last, with none.
        let laid = laid(&h, &SOLID_PANELS, false);
        panels_placed(&h, &laid);
        // Each button 30 square, 1 apart, centred under the panel's 4; its 20 point icon
        // (`svg.i.lg`) centred, Gray.750 (`.tb-btn`), or Gray.400 off (`:disabled`): Pocket,
        // whose dialog can't open, and what isn't built. SELECT's filters, each on (every
        // kind taken, as the filter starts), are `.tb-btn.toggle-on`: white on Gray.250, a
        // 2 point blue line inside its foot (`box-shadow: inset 0 -2px 0 var(--blue)`).
        let on = [
            "Create Sketch",
            "Pad",
            "Revolve",
            "Groove",
            "Fillet",
            "Chamfer",
            "Union",
            "Subtract",
            "Intersect",
        ];
        let toggles = ["Faces", "Edges", "Vertices"];
        let icon = |rect: Rect| Rect::from_center_size(rect.center(), Vec2::splat(css::ICON_LG));
        for (label, rect) in laid.iter().flat_map(|panel| panel.buttons.iter()) {
            sized(widget(&h, Role::Button, label), *rect, label);
            if toggles.contains(label) {
                assert_eq!(icon_at(&h, icon(*rect)), Some(css::G1000), "{label}'s icon");
                assert_eq!(
                    fill_at(&h, *rect),
                    Some((css::G250, CornerRadius::same(css::R_SM))),
                    "{label}'s fill"
                );
                let foot = rect.bottom() - 1.0;
                assert_eq!(
                    lines(
                        &h,
                        Pos2::new(rect.left(), foot),
                        Pos2::new(rect.right(), foot)
                    ),
                    [Stroke::new(2.0, css::BLUE)],
                    "{label}'s foot"
                );
                continue;
            }
            let ink = if on.contains(label) {
                css::G750
            } else {
                css::G400
            };
            assert_eq!(icon_at(&h, icon(*rect)), Some(ink), "{label}'s icon");
            assert_eq!(fill_at(&h, *rect), None, "{label}'s fill");
        }
        // Hovered (`.tb-btn:hover:not(:disabled)`): Gray.300 behind white, its corners
        // `--r-sm`; Pocket, off, unchanged.
        let pad = laid[0].buttons[1].1;
        hover(&mut h, Role::Button, "Pad");
        assert_eq!(
            fill_at(&h, pad),
            Some((css::G300, CornerRadius::same(css::R_SM)))
        );
        assert_eq!(icon_at(&h, icon(pad)), Some(css::G1000));
        let pocket = laid[0].buttons[2].1;
        hover(&mut h, Role::Button, "Pocket");
        assert_eq!(fill_at(&h, pocket), None);
        assert_eq!(icon_at(&h, icon(pocket)), Some(css::G400));
    }

    #[test]
    fn the_drop_downs_labels_are_drawn_as_tb_label() {
        let mut h = drawn(false);
        // Each label centred under its row, 1 below it: 4, its name, 2, its 8 point arrow,
        // and 4, on a 14 line in 1 above and below; its name 10 points, Gray.550, its arrow
        // too, centred on the label's middle.
        let laid = laid(&h, &SOLID_PANELS, false);
        let check = |h: &Harness<'_, bool>, panel: &Laid, ink: Color32| {
            let rect = h
                .get_by_role_and_label(Role::Toolbar, panel.name)
                .get_by_role_and_label(Role::Button, panel.name)
                .rect();
            sized(rect, panel.label, panel.name);
            let shown = text(h, panel.name);
            capitals(&shown, css::LABEL_FS, Some(css::LABEL_LH));
            assert_eq!(colour(&shown), ink, "{}", panel.name);
            at(
                shown.pos,
                panel.label.min + Vec2::new(css::LABEL_PADDING_X, css::LABEL_PADDING_Y),
                panel.name,
            );
            assert_eq!(shown.galley.size().y, css::LABEL_LH, "{}", panel.name);
            let arrow = Rect::from_min_size(
                Pos2::new(
                    shown.pos.x + shown.galley.size().x + css::LABEL_GAP,
                    panel.label.center().y - css::LABEL_ARROW / 2.0,
                ),
                Vec2::splat(css::LABEL_ARROW),
            );
            assert_eq!(icon_at(h, arrow), Some(ink), "{}'s arrow", panel.name);
        };
        for panel in &laid {
            check(&h, panel, css::G550);
            assert_eq!(fill_at(&h, panel.label), None, "{}", panel.name);
        }
        // Hovered (`.tb-label:hover`): Gray.250 behind white, its corners 3.
        h.get_by_role_and_label(Role::Toolbar, "MODIFY")
            .get_by_role_and_label(Role::Button, "MODIFY")
            .hover();
        h.run_steps(2);
        check(&h, &laid[1], css::G1000);
        assert_eq!(
            fill_at(&h, laid[1].label),
            Some((css::G250, CornerRadius::same(css::LABEL_RADIUS)))
        );
    }

    #[test]
    fn the_sketch_tabs_panels_and_finish_sketch_are_drawn_as_tb_panel_and_btn_finish() {
        let mut h = drawn(true);
        // SKETCH's panels, INSPECT's border kept: Finish Sketch comes after it.
        let laid = laid(&h, &SKETCH_PANELS, true);
        panels_placed(&h, &laid);
        for (label, rect) in laid.iter().flat_map(|panel| panel.buttons.iter()) {
            sized(widget(&h, Role::Button, label), *rect, label);
        }
        // `.tb-finish`: at the panels' right, in `.tb-panels`' 6 and its own 8, 30 high on
        // the panels' middle; `.btn-finish`: 12, the check (`svg.i`, 14), 6, its name in
        // 12 points, and 12; blue behind white, its corners `--r-sm`.
        let name = text(&h, "Finish Sketch");
        assert_eq!(format(&name).font_id, FontId::proportional(css::FS));
        assert_eq!(colour(&name), css::G1000);
        let right = WIDTH - css::PANELS_PADDING - css::FINISH_MARGIN;
        let width = css::FINISH_PADDING
            + css::ICON
            + css::FINISH_GAP
            + name.galley.size().x
            + css::FINISH_PADDING;
        let middle = css::TABS_H + css::PANELS_H / 2.0;
        let finish = Rect::from_min_max(
            Pos2::new(right - width, middle - css::FINISH_H / 2.0),
            Pos2::new(right, middle + css::FINISH_H / 2.0),
        );
        sized(
            widget(&h, Role::Button, "Finish Sketch"),
            finish,
            "Finish Sketch",
        );
        assert_eq!(
            fill_at(&h, finish),
            Some((css::BLUE, CornerRadius::same(css::R_SM)))
        );
        let check = Rect::from_min_size(
            Pos2::new(
                finish.left() + css::FINISH_PADDING,
                middle - css::ICON / 2.0,
            ),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, check), Some(css::G1000));
        at(
            name.pos,
            Pos2::new(
                check.right() + css::FINISH_GAP,
                middle - name.galley.size().y / 2.0,
            ),
            "Finish Sketch",
        );
        // Hovered (`.btn-finish:hover`): #2a78e8.
        hover(&mut h, Role::Button, "Finish Sketch");
        assert_eq!(
            fill_at(&h, finish),
            Some((css::FINISH_HOVER, CornerRadius::same(css::R_SM)))
        );
    }

    /// SKETCH's buttons and what each does: a tool chosen (the rectangle the corner one,
    /// the arc the 3-point one, as their keys choose them), Construction turned on or off,
    /// a relation made of the selection, or nothing yet.
    const SKETCH_DOES: [(&str, Option<Act>); 21] = [
        ("Line", Some(Act::SketchTool(Choice::Draw(ToolKind::Line)))),
        (
            "Rectangle",
            Some(Act::SketchTool(Choice::Draw(ToolKind::CornerRectangle))),
        ),
        (
            "Circle",
            Some(Act::SketchTool(Choice::Draw(ToolKind::Circle))),
        ),
        (
            "Arc",
            Some(Act::SketchTool(Choice::Draw(ToolKind::ThreePointArc))),
        ),
        (
            "Point",
            Some(Act::SketchTool(Choice::Draw(ToolKind::Point))),
        ),
        ("Construction", Some(Act::Construction)),
        ("Trim", Some(Act::SketchTool(Choice::Trim))),
        ("Extend", None),
        ("Split", Some(Act::SketchTool(Choice::Split))),
        ("Smart Dimension", Some(Act::SketchTool(Choice::Dimension))),
        ("Horizontal", Some(Act::Relate(RelationTool::Horizontal))),
        ("Vertical", Some(Act::Relate(RelationTool::Vertical))),
        ("Coincident", Some(Act::Relate(RelationTool::Coincident))),
        ("Tangent", Some(Act::Relate(RelationTool::Tangent))),
        ("Equal", Some(Act::Relate(RelationTool::Equal))),
        ("Parallel", Some(Act::Relate(RelationTool::Parallel))),
        (
            "Perpendicular",
            Some(Act::Relate(RelationTool::Perpendicular)),
        ),
        ("Midpoint", Some(Act::Relate(RelationTool::Midpoint))),
        ("Symmetric", Some(Act::Relate(RelationTool::Symmetric))),
        ("Fix", Some(Act::Relate(RelationTool::Fix))),
        ("Measure", None),
    ];

    #[test]
    fn each_sketch_button_asks_its_tool_construction_or_relation_and_what_isnt_built_is_off() {
        let sketching = Sketching::default();
        let mut h = drawn_with(true, &sketching);
        let laid = laid(&h, &SKETCH_PANELS, true);
        let icon = |rect: Rect| Rect::from_center_size(rect.center(), Vec2::splat(css::ICON_LG));
        let buttons: Vec<(&str, Rect)> = laid
            .iter()
            .flat_map(|panel| panel.buttons.iter().copied())
            .collect();
        assert_eq!(buttons.len(), SKETCH_DOES.len());
        for ((label, rect), (named, does)) in buttons.iter().zip(SKETCH_DOES) {
            assert_eq!(*label, named);
            // Gray.750 on, Gray.400 off (`:disabled`); none drawn pressed with no tool on
            // and Construction off, each that can be pressed (a tool, Construction; never a
            // relation, a click of which is one edit) saying it isn't.
            let ink = if does.is_some() { css::G750 } else { css::G400 };
            assert_eq!(icon_at(&h, icon(*rect)), Some(ink), "{label}'s icon");
            assert_eq!(fill_at(&h, *rect), None, "{label}'s fill");
            let node = h.get_by_role_and_label(Role::Button, label);
            assert_eq!(
                node.accesskit_node().is_disabled(),
                does.is_none(),
                "{label}"
            );
            let pressed = node.accesskit_node().toggled();
            let pressable = matches!(does, Some(Act::SketchTool(_) | Act::Construction));
            assert_eq!(pressed.is_some(), pressable, "{label} pressable");
        }
        for (label, does) in SKETCH_DOES {
            sketching.asked.set(None);
            h.get_by_role_and_label(Role::Button, label).click();
            h.run_steps(2);
            assert_eq!(sketching.asked.get(), does, "{label}");
        }
    }

    #[test]
    fn the_tool_on_is_drawn_as_tb_btn_on_and_construction_on_as_toggle_on_hover_over_both() {
        let sketching = Sketching::default();
        sketching
            .tool
            .set(Some(Choice::Draw(ToolKind::CornerRectangle)));
        sketching.construction.set(true);
        let mut h = drawn_with(true, &sketching);
        let laid = laid(&h, &SKETCH_PANELS, true);
        let rect_of = |label: &str| {
            laid.iter()
                .flat_map(|panel| panel.buttons.iter())
                .find(|(name, _)| *name == label)
                .map(|(_, rect)| *rect)
                .unwrap()
        };
        let icon = |rect: Rect| Rect::from_center_size(rect.center(), Vec2::splat(css::ICON_LG));
        let toggled = |h: &Harness<'_, bool>, label: &str| {
            h.get_by_role_and_label(Role::Button, label)
                .accesskit_node()
                .toggled()
        };
        // `.tb-btn.on`: `--blue` behind `--blue-text`, its corners `--r-sm`.
        let rectangle = rect_of("Rectangle");
        assert_eq!(
            fill_at(&h, rectangle),
            Some((css::BLUE, CornerRadius::same(css::R_SM)))
        );
        assert_eq!(icon_at(&h, icon(rectangle)), Some(css::BLUE_TEXT));
        assert_eq!(toggled(&h, "Rectangle"), Some(Toggled::True));
        // The other tools at rest, saying they're not on.
        for label in [
            "Line",
            "Circle",
            "Arc",
            "Point",
            "Trim",
            "Split",
            "Smart Dimension",
        ] {
            assert_eq!(fill_at(&h, rect_of(label)), None, "{label}");
            assert_eq!(
                icon_at(&h, icon(rect_of(label))),
                Some(css::G750),
                "{label}"
            );
            assert_eq!(toggled(&h, label), Some(Toggled::False), "{label}");
        }
        // `.tb-btn.toggle-on`: `--g250` behind white, and `inset 0 -2px 0 var(--blue)`: a
        // 2 point blue line inside its foot, as SELECT's filters draw it.
        let construction = rect_of("Construction");
        assert_eq!(
            fill_at(&h, construction),
            Some((css::G250, CornerRadius::same(css::R_SM)))
        );
        assert_eq!(icon_at(&h, icon(construction)), Some(css::G1000));
        let foot = construction.bottom() - 1.0;
        let inset = |h: &Harness<'_, bool>| {
            lines(
                h,
                Pos2::new(construction.left(), foot),
                Pos2::new(construction.right(), foot),
            )
        };
        assert_eq!(inset(&h), [Stroke::new(2.0, css::BLUE)]);
        assert_eq!(toggled(&h, "Construction"), Some(Toggled::True));
        // Hovered while on: `.tb-btn:hover` sets only the background and the colour,
        // Gray.300 behind white, and `.toggle-on`'s box-shadow stays.
        hover(&mut h, Role::Button, "Construction");
        assert_eq!(
            fill_at(&h, construction),
            Some((css::G300, CornerRadius::same(css::R_SM)))
        );
        assert_eq!(icon_at(&h, icon(construction)), Some(css::G1000));
        assert_eq!(inset(&h), [Stroke::new(2.0, css::BLUE)], "hovered");
        // Construction off: at rest, saying so, and no inset line.
        sketching.construction.set(false);
        hover(&mut h, Role::Button, "Line");
        assert_eq!(fill_at(&h, construction), None);
        assert!(inset(&h).is_empty());
        assert_eq!(icon_at(&h, icon(construction)), Some(css::G750));
        assert_eq!(toggled(&h, "Construction"), Some(Toggled::False));
        // Hovered (`.tb-btn:hover:not(:disabled)`, which outweighs `.on`): Gray.300 behind
        // white.
        hover(&mut h, Role::Button, "Rectangle");
        assert_eq!(
            fill_at(&h, rectangle),
            Some((css::G300, CornerRadius::same(css::R_SM)))
        );
        assert_eq!(icon_at(&h, icon(rectangle)), Some(css::G1000));
        // Another tool on: Rectangle at rest, Split drawn on.
        sketching.tool.set(Some(Choice::Split));
        hover(&mut h, Role::Button, "Line");
        assert_eq!(fill_at(&h, rectangle), None);
        assert_eq!(toggled(&h, "Rectangle"), Some(Toggled::False));
        assert_eq!(
            fill_at(&h, rect_of("Split")),
            Some((css::BLUE, CornerRadius::same(css::R_SM)))
        );
    }

    /// A button the registry gives keys is the registry's command, under its name and its
    /// line, so its tooltip and the sheet say the same: the keys that reach it do what it
    /// does.
    #[test]
    fn each_keyed_sketch_button_is_its_registry_commands_under_its_name() {
        let commands: Vec<&Command> = SKETCH.iter().flat_map(|panel| panel.row.iter()).collect();
        for command in &commands {
            if let Some(key) = command.key {
                assert_eq!(
                    command.description,
                    key.entry().description,
                    "{}",
                    command.label
                );
            }
        }
        let keyed: Vec<(&str, shortcuts::Command)> = commands
            .iter()
            .filter_map(|command| command.key.map(|key| (command.label, key)))
            .collect();
        assert_eq!(
            keyed,
            [
                ("Line", shortcuts::Command::Line),
                ("Rectangle", shortcuts::Command::Rectangle),
                ("Circle", shortcuts::Command::Circle),
                ("Arc", shortcuts::Command::Arc),
                ("Point", shortcuts::Command::Point),
                ("Construction", shortcuts::Command::Construction),
                ("Trim", shortcuts::Command::Trim),
                ("Smart Dimension", shortcuts::Command::Dimension),
            ]
        );
        for (label, key) in keyed {
            assert_eq!(key.label(), label);
        }
    }

    #[test]
    fn a_filters_tooltip_names_its_key_and_its_click_toggles_it() {
        // SELECT's Edges: its tooltip names E, the registry's key (the prototype's
        // `data-tipk`); a click asks to toggle edges.
        let mut h = drawn(false);
        let painted = |h: &Harness<'_, bool>, text: &str| {
            clipped(h)
                .into_iter()
                .filter(|shown| matches!(&shown.shape, Shape::Text(t) if t.galley.text() == text))
                .count()
        };
        assert_eq!(painted(&h, "E"), 0);
        h.get_by_role_and_label(Role::Button, "Edges").hover();
        h.run_steps(8);
        assert_eq!(painted(&h, "E"), 1, "the tooltip's key cap");
        let act = std::cell::Cell::new(None);
        let mut h = Harness::builder()
            .with_size(Vec2::new(WIDTH, 200.0))
            .build_ui(|ui| {
                let can_open = |_: Kind| true;
                let shown = Shown {
                    in_sketch: false,
                    create_sketch: Some(true),
                    can_open: &can_open,
                    why_closed: &|_| None,
                    selected: false,
                    filter: Filter::default(),
                    tool: None,
                    construction: false,
                };
                let window = UiBuilder::new().max_rect(ui.ctx().content_rect());
                if let Some(done) = ui.scope_builder(window, |ui| show(ui, &shown)).inner {
                    act.set(Some(done));
                }
            });
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Edges").click();
        h.run_steps(2);
        drop(h);
        assert_eq!(act.get(), Some(Act::Filter(FilterKind::Edges)));
    }
}
