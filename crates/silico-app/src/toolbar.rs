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
    self, Align, Color32, FontId, Galley, Key, KeyboardShortcut, Layout, Modifiers, Pos2, Rect,
    Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
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

/// A panel: its label, its row of buttons, and its drop-down: its own entries, or, with
/// none, its row's commands.
struct Panel {
    label: &'static str,
    row: &'static [Command],
    menu: Option<&'static [Entry]>,
}

/// SOLID's panels (app.js:394, interact.js:170): the primitives in CREATE's drop-down alone.
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
            Entry::Separator,
            Entry::Heading("PRIMITIVES"),
            Entry::Command(BOX),
            Entry::Command(CYLINDER),
            Entry::Command(SPHERE),
            Entry::Command(CONE),
            Entry::Command(TORUS),
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

/// SKETCH's panels (app.js:384), before Finish Sketch.
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
            menu_node(ui, panel.label, |ui| match panel.menu {
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
    // Painted at the toolbar's opacity, not faded further when off: `.tb-btn:disabled` is
    // Gray.400 itself.
    let opacity = ui.opacity();
    ui.add_enabled(enabled, |ui: &mut Ui| {
        ui.set_opacity(opacity);
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
    ui.painter().galley(text, galley, color);
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
                    .extra_letter_spacing(theme::CAPITALS_LETTER_SPACING),
            );
        }
        Entry::AllFilters => {
            ui.add_enabled(false, egui::Button::new("Select all filters"))
                .on_disabled_hover_text(NOT_BUILT);
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
                Does::CreateSketch => {
                    let (button, _) = ui
                        .add_enabled_ui(enabled, |ui| {
                            egui::containers::menu::SubMenuButton::from_button(item)
                                .ui(ui, |ui| menu_node(ui, command.label, |ui| planes(ui, act)))
                        })
                        .inner;
                    if let (false, Some(said)) = (command.built(shown), command.said(shown)) {
                        button.on_disabled_hover_text(said);
                    }
                }
                Does::Dialog(kind) => {
                    if ui.add_enabled(enabled, item).clicked() {
                        *act = Some(Act::Dialog(kind));
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
        Some(crate::shortcuts::Chord::Key(KeyboardShortcut::new(
            Modifiers::COMMAND,
            Key::Enter,
        ))),
        Some("Keeps your changes and rebuilds"),
    )
}

#[cfg(test)]
mod tests {
    use eframe::egui::epaint::{ClippedShape, RectShape, TextShape};
    use eframe::egui::{CornerRadius, Shape, TextFormat};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// A dialog's command is labelled with its kind's title, which the dialog's heading
    /// and the tests find it by; every kind is in CREATE's drop-down once, and the sweeps
    /// in its row too (app.js:394), the primitives not.
    #[test]
    fn every_dialog_is_in_creates_drop_down_under_its_title_and_the_sweeps_in_its_row() {
        let create = &SOLID[0];
        let menu = create.menu.unwrap_or_default();
        for kind in Kind::ALL {
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
            assert_eq!(in_row, usize::from(kind.sweeps()), "{kind:?}");
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
        let mut h = Harness::builder()
            .with_size(Vec2::new(WIDTH, 200.0))
            .build_ui_state(
                |ui, in_sketch: &mut bool| {
                    let can_open = |kind: Kind| kind != Kind::Pocket;
                    let shown = Shown {
                        in_sketch: *in_sketch,
                        create_sketch: Some(true),
                        can_open: &can_open,
                        selected: false,
                    };
                    let window = UiBuilder::new().max_rect(ui.ctx().content_rect());
                    ui.scope_builder(window, |ui| show(ui, &shown));
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
        // whose dialog can't open, and what isn't built.
        let on = ["Create Sketch", "Pad", "Revolve", "Groove"];
        let icon = |rect: Rect| Rect::from_center_size(rect.center(), Vec2::splat(css::ICON_LG));
        for (label, rect) in laid.iter().flat_map(|panel| panel.buttons.iter()) {
            sized(widget(&h, Role::Button, label), *rect, label);
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
}
