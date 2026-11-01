//! The toolbar, as the approved UI prototype draws it (docs/design/m5-ui/DESIGN.md, *Layout*
//! and *egui mapping*; styles.css's `.tb-*` and `.btn-finish`; app.js's `renderToolbar` and
//! its command table, interact.js's `createMenu`, `modifyMenu`, and `selectMenu`): a row of
//! tabs, the Design workspace's SOLID and, while a sketch is open, the contextual SKETCH, in
//! blue; under it the tab's panels, each a row of icon buttons over its capital label, a
//! drop-down; and, in SKETCH, a blue Finish Sketch at the row's right end.
//!
//! Only what the app can do is on: the feature dialogs (M5.8), and clearing the selection.
//! The prototype's other commands keep their places, drawn off, their tooltips saying they
//! aren't built yet. What the toolbar shows comes in a [`Shown`], and what was clicked goes
//! back as an [`Act`], so a test drives it with a stub, and sketch mode's state (whether a
//! sketch is open, whether Create Sketch can open) and its actions (Create Sketch on a plane,
//! Finish Sketch) are the caller's to wire.

use std::sync::Arc;

use eframe::egui::{
    self, Align, FontId, Galley, Key, KeyboardShortcut, Layout, Modifiers, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
    accesskit::Role, text::LayoutJob,
};
use silico_core::document::OriginPlane;

use crate::bar;
use crate::dialog::Kind;
use crate::icons::{self, ICON_SIZE, Icon, IconSize, TOOLBAR_ICON_SIZE};
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
    /// Whether anything is selected, for SELECT's Clear selection.
    pub selected: bool,
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
    /// SELECT's Clear selection.
    ClearSelection,
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
    /// Nothing yet: drawn off.
    NotBuilt,
}

/// A command as the toolbar offers it: its name (its button's label and its tooltip's
/// title), its icon, the prototype's line on it, and what it does.
#[derive(Debug, Clone, Copy)]
struct Command {
    label: &'static str,
    icon: Icon,
    description: Option<&'static str>,
    does: Does,
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
    }
}

/// What a command's tooltip says when it isn't built yet.
const NOT_BUILT: &str = "Not built yet";

const CREATE_SKETCH: Command = command(
    "Create Sketch",
    Icon::Sketch,
    Some("Pick an origin plane or a flat face, then sketch on it"),
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
const BOX: Command = command("Box", Icon::Box, None, Does::Dialog(Kind::Box));
const CYLINDER: Command = command(
    "Cylinder",
    Icon::Cylinder,
    None,
    Does::Dialog(Kind::Cylinder),
);
const SPHERE: Command = command("Sphere", Icon::Sphere, None, Does::Dialog(Kind::Sphere));
const CONE: Command = command("Cone", Icon::Cone, None, Does::Dialog(Kind::Cone));
const TORUS: Command = command("Torus", Icon::Torus, None, Does::Dialog(Kind::Torus));
const FILLET: Command = command(
    "Fillet",
    Icon::Fillet,
    Some("Rounds the selected edges"),
    Does::NotBuilt,
);
const CHAMFER: Command = command(
    "Chamfer",
    Icon::Chamfer,
    Some("Bevels the selected edges"),
    Does::NotBuilt,
);
const UNION: Command = command("Union", Icon::Union, None, Does::NotBuilt);
const SUBTRACT: Command = command("Subtract", Icon::Subtract, None, Does::NotBuilt);
const INTERSECT: Command = command("Intersect", Icon::Intersect, None, Does::NotBuilt);
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
    Does::NotBuilt,
);
const EDGES: Command = command(
    "Edges",
    Icon::SelEdge,
    Some("Toggles whether edges can be selected"),
    Does::NotBuilt,
);
const VERTICES: Command = command(
    "Vertices",
    Icon::SelVertex,
    Some("Toggles whether vertices can be selected"),
    Does::NotBuilt,
);
const LINE: Command = command(
    "Line",
    Icon::Line,
    Some("Click, click: a chain of lines. Double-click ends it"),
    Does::NotBuilt,
);
const RECTANGLE: Command = command("Rectangle", Icon::Rect, None, Does::NotBuilt);
const CIRCLE: Command = command("Circle", Icon::Circle, None, Does::NotBuilt);
const ARC: Command = command("Arc", Icon::Arc, None, Does::NotBuilt);
const POINT: Command = command("Point", Icon::Point, None, Does::NotBuilt);
const CONSTRUCTION: Command = command(
    "Construction",
    Icon::Construction,
    Some("Toggles the selected curves between profile and construction"),
    Does::NotBuilt,
);
const TRIM: Command = command("Trim", Icon::Trim, None, Does::NotBuilt);
const EXTEND: Command = command("Extend", Icon::Extend, None, Does::NotBuilt);
const SPLIT: Command = command("Split", Icon::Split, None, Does::NotBuilt);
const DIMENSION: Command = command(
    "Smart Dimension",
    Icon::Dimension,
    Some("Pick a line, a circle, or two entities, then type the value"),
    Does::NotBuilt,
);

/// A constraint's button (app.js:387): its name, and the prototype's line on them all.
const fn constraint(label: &'static str, icon: Icon) -> Command {
    command(
        label,
        icon,
        Some("Select the curves or points first"),
        Does::NotBuilt,
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

/// A panel: its label, its row of buttons, and its drop-down.
struct Panel {
    label: &'static str,
    row: &'static [Command],
    menu: &'static [Entry],
}

/// SOLID's panels (app.js:394, interact.js:170). CREATE's row is the prototype's, then the
/// primitives, which the prototype keeps in its drop-down alone.
const SOLID: &[Panel] = &[
    Panel {
        label: "CREATE",
        row: &[
            CREATE_SKETCH,
            PAD,
            POCKET,
            REVOLVE,
            GROOVE,
            BOX,
            CYLINDER,
            SPHERE,
            CONE,
            TORUS,
        ],
        menu: &[
            Entry::Command(CREATE_SKETCH),
            Entry::Separator,
            Entry::Command(PAD),
            Entry::Command(POCKET),
            Entry::Command(REVOLVE),
            Entry::Command(GROOVE),
            Entry::Separator,
            Entry::Heading("PRIMITIVES"),
            Entry::Command(BOX),
            Entry::Command(CYLINDER),
            Entry::Command(SPHERE),
            Entry::Command(CONE),
            Entry::Command(TORUS),
        ],
    },
    Panel {
        label: "MODIFY",
        row: &[FILLET, CHAMFER, UNION, SUBTRACT, INTERSECT],
        menu: &[
            Entry::Command(FILLET),
            Entry::Command(CHAMFER),
            Entry::Separator,
            Entry::Heading("COMBINE"),
            Entry::Command(UNION),
            Entry::Command(SUBTRACT),
            Entry::Command(INTERSECT),
        ],
    },
    Panel {
        label: "INSPECT",
        row: &[MEASURE],
        menu: &[Entry::Command(MEASURE)],
    },
    Panel {
        label: "SELECT",
        row: &[FACES, EDGES, VERTICES],
        menu: &[
            Entry::Heading("SELECTION FILTERS"),
            Entry::Command(FACES),
            Entry::Command(EDGES),
            Entry::Command(VERTICES),
            Entry::Separator,
            Entry::AllFilters,
            Entry::ClearSelection,
        ],
    },
];

/// SKETCH's panels (app.js:384), before Finish Sketch.
const SKETCH: &[Panel] = &[
    Panel {
        label: "CREATE",
        row: &[LINE, RECTANGLE, CIRCLE, ARC, POINT, CONSTRUCTION],
        menu: &[
            Entry::Command(LINE),
            Entry::Command(RECTANGLE),
            Entry::Command(CIRCLE),
            Entry::Command(ARC),
            Entry::Command(POINT),
            Entry::Command(CONSTRUCTION),
        ],
    },
    Panel {
        label: "MODIFY",
        row: &[TRIM, EXTEND, SPLIT],
        menu: &[
            Entry::Command(TRIM),
            Entry::Command(EXTEND),
            Entry::Command(SPLIT),
        ],
    },
    Panel {
        label: "CONSTRAINTS",
        row: &CONSTRAINTS,
        menu: &[
            Entry::Command(CONSTRAINTS[0]),
            Entry::Command(CONSTRAINTS[1]),
            Entry::Command(CONSTRAINTS[2]),
            Entry::Command(CONSTRAINTS[3]),
            Entry::Command(CONSTRAINTS[4]),
            Entry::Command(CONSTRAINTS[5]),
            Entry::Command(CONSTRAINTS[6]),
            Entry::Command(CONSTRAINTS[7]),
            Entry::Command(CONSTRAINTS[8]),
            Entry::Command(CONSTRAINTS[9]),
            Entry::Command(CONSTRAINTS[10]),
        ],
    },
    Panel {
        label: "INSPECT",
        row: &[MEASURE],
        menu: &[Entry::Command(MEASURE)],
    },
];

/// CONSTRAINTS' row: Smart Dimension, then the constraints in app.js:387's order.
const CONSTRAINTS: [Command; 11] = [
    DIMENSION,
    constraint("Horizontal", Icon::CHorizontal),
    constraint("Vertical", Icon::CVertical),
    constraint("Coincident", Icon::CCoincident),
    constraint("Tangent", Icon::CTangent),
    constraint("Equal", Icon::CEqual),
    constraint("Parallel", Icon::CParallel),
    constraint("Perpendicular", Icon::CPerpendicular),
    constraint("Midpoint", Icon::CMidpoint),
    constraint("Symmetric", Icon::CSymmetric),
    constraint("Fix", Icon::CFix),
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
            Does::NotBuilt => false,
        }
    }

    /// Whether the app has it.
    fn built(&self, shown: &Shown<'_>) -> bool {
        match self.does {
            Does::Dialog(_) => true,
            Does::CreateSketch => shown.create_sketch.is_some(),
            Does::NotBuilt => false,
        }
    }

    /// Its tooltip's line: the prototype's, and that it isn't built yet if it isn't.
    fn said(&self, shown: &Shown<'_>) -> Option<String> {
        match (self.description, self.built(shown)) {
            (description, true) => description.map(str::to_owned),
            (Some(description), false) => Some(format!("{description}. {NOT_BUILT}")),
            (None, false) => Some(NOT_BUILT.to_owned()),
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

/// `text` in the toolbar's capitals, `size` points, on a `line` high line.
fn capitals(ui: &Ui, text: &str, size: f32, line: Option<f32>) -> Arc<Galley> {
    let job = LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat {
            font_id: FontId::proportional(size),
            extra_letter_spacing: theme::TOOLBAR_LETTER_SPACING,
            line_height: line,
            color: theme::TEXT_DEFAULT,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// The workspace's name, DESIGN (`.tb-ws`): the row's height, its text at its bottom, a
/// border at its right.
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
        rect.bottom() - theme::TOOLBAR_WORKSPACE_PADDING_BOTTOM - galley.size().y,
    );
    let painter = ui.painter();
    painter.galley_with_override_text_color(at, galley, theme::TOOLBAR_WORKSPACE_TEXT);
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
    painter.galley_with_override_text_color(at, galley, text);
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
    let width = row_width.max(label_size.x) + 2.0 * theme::TOOLBAR_PANEL_PADDING_SIDE;
    let rect = Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(width, theme::TOOLBAR_PANELS_HEIGHT),
    );
    let row = Rect::from_center_size(
        Pos2::new(
            rect.center().x,
            rect.top() + theme::TOOLBAR_PANEL_PADDING_TOP + theme::TOOLBAR_BUTTON_SIZE / 2.0,
        ),
        Vec2::new(row_width, theme::TOOLBAR_BUTTON_SIZE),
    );
    let label_rect = Rect::from_center_size(
        Pos2::new(
            rect.center().x,
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
            menu_node(ui, panel.label, |ui| {
                for entry in panel.menu {
                    menu_entry(ui, entry, shown, act);
                }
            });
        });
    });
    if !last {
        ui.painter().vline(
            rect.right() - 0.5,
            rect.y_range(),
            Stroke::new(theme::BORDER_WIDTH, theme::TOOLBAR_DIVIDER),
        );
    }
}

/// A command's button in a panel's row, with its tooltip; a click opens its dialog, or, for
/// Create Sketch, its planes.
fn row_button(ui: &mut Ui, command: &Command, shown: &Shown<'_>, act: &mut Option<Act>) {
    let response = tool_button(ui, command.icon, command.label, command.enabled(shown));
    let said = command.said(shown);
    let response = bar::tooltip(response, command.label, None, said.as_deref());
    match command.does {
        Does::Dialog(kind) if response.clicked() => *act = Some(Act::Dialog(kind)),
        Does::CreateSketch => bar::menu(&response, |ui| {
            menu_node(ui, command.label, |ui| planes(ui, act));
        }),
        Does::Dialog(_) | Does::NotBuilt => {}
    }
}

/// A toolbar button (`.tb-btn`): `icon`, labelled `label`, off unless `enabled`.
fn tool_button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    ui.add_enabled(enabled, |ui: &mut Ui| {
        let size = Vec2::splat(theme::TOOLBAR_BUTTON_SIZE);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let color = if !enabled {
            theme::TOOLBAR_BUTTON_DISABLED_ICON
        } else if response.hovered() {
            ui.painter().rect_filled(
                rect,
                theme::TOOLBAR_BUTTON_CORNER_RADIUS,
                theme::TOOLBAR_BUTTON_HOVER_BG,
            );
            theme::TOOLBAR_BUTTON_HOVER_ICON
        } else {
            theme::TOOLBAR_BUTTON_ICON
        };
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
    ui.painter()
        .galley_with_override_text_color(text, galley, color);
    Icon::Down
        .image(IconSize::Small)
        .fit_to_exact_size(arrow.size())
        .tint(color)
        .paint_at(ui, arrow);
    response
}

/// `add` in a menu for a screen reader, labelled `label`.
fn menu_node(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.scope_builder(UiBuilder::new().id_salt(label), |ui| {
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::Menu);
            node.set_label(label);
        });
        add(ui);
    });
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
                    .extra_letter_spacing(theme::TOOLBAR_LETTER_SPACING),
            );
        }
        Entry::AllFilters => {
            ui.add_enabled(false, egui::Button::new("Select all filters"))
                .on_disabled_hover_text(NOT_BUILT);
        }
        Entry::ClearSelection => {
            let clear = egui::Button::new("Clear selection").shortcut_text("Esc");
            if ui.add_enabled(shown.selected, clear).clicked() {
                *act = Some(Act::ClearSelection);
                ui.close();
            }
        }
        Entry::Command(command) => {
            let item = egui::Button::image_and_text(icons::icon(command.icon), command.label);
            let enabled = command.enabled(shown);
            match command.does {
                Does::CreateSketch => {
                    let (button, _) = ui
                        .add_enabled_ui(enabled, |ui| {
                            egui::containers::menu::SubMenuButton::from_button(item)
                                .ui(ui, |ui| menu_node(ui, command.label, |ui| planes(ui, act)))
                        })
                        .inner;
                    if !command.built(shown) {
                        button.on_disabled_hover_text(NOT_BUILT);
                    }
                }
                Does::Dialog(kind) => {
                    if ui.add_enabled(enabled, item).clicked() {
                        *act = Some(Act::Dialog(kind));
                        ui.close();
                    }
                }
                Does::NotBuilt => {
                    ui.add_enabled(false, item)
                        .on_disabled_hover_text(NOT_BUILT);
                }
            }
        }
    }
}

/// Create Sketch's planes: a click on one starts a sketch on it.
fn planes(ui: &mut Ui, act: &mut Option<Act>) {
    for (plane, word) in PLANES {
        if ui.button(word).clicked() {
            *act = Some(Act::CreateSketch(plane));
            ui.close();
        }
    }
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
    bar::tooltip(
        response,
        label,
        Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter)),
        Some("Keeps your changes and rebuilds"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialog's command is labelled with its kind's title, which the dialog's heading
    /// and the tests find it by; every kind is in CREATE's row once, and in its drop-down.
    #[test]
    fn every_dialog_is_in_creates_row_and_drop_down_under_its_title() {
        let create = &SOLID[0];
        for kind in Kind::ALL {
            let in_row: Vec<_> = create
                .row
                .iter()
                .filter(|command| command.does == Does::Dialog(kind))
                .collect();
            assert_eq!(in_row.len(), 1, "{kind:?}");
            assert_eq!(in_row[0].label, kind.title());
            assert!(create.menu.iter().any(
                |entry| matches!(entry, Entry::Command(command) if command.does == Does::Dialog(kind))
            ));
        }
    }
}
