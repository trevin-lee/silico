//! Feature dialogs (M5.8; docs/design/m5.0-app.md, sections 8 and 9, *Feature dialogs*):
//! Fusion's docked dialog, a vertical stack of labelled inputs with OK and Cancel at its
//! bottom, of the primitives and of the features that sweep a sketch's profile. A dialog
//! holds what its inputs say; the tab turns each change into an edit of the document
//! inside its transaction ([`crate::tab::Tab`]), so the preview is the model rebuilt.
//!
//! Every input sets one part of one field of the feature (`set_field`'s field): a number,
//! a choice, a toggle, or a selection box. A box of a feature (a sweep's sketch) the browser
//! fills. A box of elements by name ([`Names`], a fillet's edges) takes the selection the
//! dialog opened with, and, while it's the active box, each click in the canvas toggles one
//! element in it, as SolidWorks' PropertyManager box takes its items (docs/ux/solidworks.md,
//! §5): the selection's pick, read against the published model, whose stored name it writes
//! ([`crate::selection::Toggled`]), or whose refusal it says.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{self, Ui};
use silico_core::Error;
use silico_core::command::Problems;
use silico_core::document::FeatureId;
use silico_core::document::json::{self, Node, Value as Json};
use silico_core::naming::Name;
use silico_protocol::ElementKind;

use crate::icons::Icon;
use crate::properties;
use crate::selection::{Filter, OnBase, Toggled};
use crate::shortcuts::Command;
use crate::theme::{self, FieldKind};

/// The features a dialog makes or edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Box,
    Cylinder,
    Sphere,
    Cone,
    Torus,
    Pad,
    Pocket,
    Revolve,
    Groove,
    Fillet,
    Chamfer,
    Union,
    Subtract,
    Intersect,
}

impl Kind {
    /// Every kind: the primitives, the sweeps, the dressings of a body's edges, then the
    /// booleans of two bodies.
    pub const ALL: [Kind; 14] = [
        Kind::Box,
        Kind::Cylinder,
        Kind::Sphere,
        Kind::Cone,
        Kind::Torus,
        Kind::Pad,
        Kind::Pocket,
        Kind::Revolve,
        Kind::Groove,
        Kind::Fillet,
        Kind::Chamfer,
        Kind::Union,
        Kind::Subtract,
        Kind::Intersect,
    ];

    /// The feature's `type`, as the document writes it.
    pub fn type_name(self) -> &'static str {
        match self {
            Kind::Box => "box",
            Kind::Cylinder => "cylinder",
            Kind::Sphere => "sphere",
            Kind::Cone => "cone",
            Kind::Torus => "torus",
            Kind::Pad => "pad",
            Kind::Pocket => "pocket",
            Kind::Revolve => "revolve",
            Kind::Groove => "groove",
            Kind::Fillet => "fillet",
            Kind::Chamfer => "chamfer",
            Kind::Union => "fuse",
            Kind::Subtract => "cut",
            Kind::Intersect => "common",
        }
    }

    /// Its name, as the toolbar and the dialog's title say it, and a new feature's name.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Box => "Box",
            Kind::Cylinder => "Cylinder",
            Kind::Sphere => "Sphere",
            Kind::Cone => "Cone",
            Kind::Torus => "Torus",
            Kind::Pad => "Pad",
            Kind::Pocket => "Pocket",
            Kind::Revolve => "Revolve",
            Kind::Groove => "Groove",
            Kind::Fillet => "Fillet",
            Kind::Chamfer => "Chamfer",
            Kind::Union => "Union",
            Kind::Subtract => "Subtract",
            Kind::Intersect => "Intersect",
        }
    }

    /// What its feature is called in a sentence, with its article: "a fillet", "a union",
    /// "an intersection".
    pub fn a_noun(self) -> &'static str {
        match self {
            Kind::Box => "a box",
            Kind::Cylinder => "a cylinder",
            Kind::Sphere => "a sphere",
            Kind::Cone => "a cone",
            Kind::Torus => "a torus",
            Kind::Pad => "a pad",
            Kind::Pocket => "a pocket",
            Kind::Revolve => "a revolve",
            Kind::Groove => "a groove",
            Kind::Fillet => "a fillet",
            Kind::Chamfer => "a chamfer",
            Kind::Union => "a union",
            Kind::Subtract => "a subtraction",
            Kind::Intersect => "an intersection",
        }
    }

    /// What its feature is called in a sentence, without its article ("union").
    pub fn noun(self) -> &'static str {
        let noun = self.a_noun();
        noun.strip_prefix("an ")
            .or_else(|| noun.strip_prefix("a "))
            .unwrap_or(noun)
    }

    /// The kind of a feature of `type_name`, if a dialog edits it.
    pub fn of(type_name: &str) -> Option<Kind> {
        Kind::ALL
            .into_iter()
            .find(|kind| kind.type_name() == type_name)
    }

    /// Whether it sweeps a sketch's profile.
    pub fn sweeps(self) -> bool {
        matches!(
            self,
            Kind::Pad | Kind::Pocket | Kind::Revolve | Kind::Groove
        )
    }

    /// Whether it cuts from a body, which it then must have.
    pub fn cuts(self) -> bool {
        matches!(self, Kind::Pocket | Kind::Groove)
    }

    /// Whether it dresses a body's edges (MODIFY's), which it then must have.
    pub fn dresses(self) -> bool {
        matches!(self, Kind::Fillet | Kind::Chamfer)
    }

    /// Whether it's a boolean of two bodies (MODIFY's COMBINE: SolidWorks' Combine, Fusion's
    /// Combine), its target the feature's `base` and its tool its `tool`, both consumed; the
    /// part then must have two bodies.
    pub fn combines(self) -> bool {
        matches!(self, Kind::Union | Kind::Subtract | Kind::Intersect)
    }

    /// Whether MODIFY offers it: the dressings and the booleans.
    pub fn modifies(self) -> bool {
        self.dresses() || self.combines()
    }

    /// Whether a new one's dialog opens with the values the last of its kind was OK'd with
    /// ([`Remembered`]): the dressings', as SolidWorks' Fillet PropertyManager "remembers
    /// its last used state" (docs/ux/solidworks.md, §5).
    pub fn remembers(self) -> bool {
        self.dresses()
    }
}

/// The values each kind of dialog that remembers ([`Kind::remembers`]) was last OK'd with
/// in the app's run, as SolidWorks' PropertyManager remembers its last used state for the
/// session: each number its inputs held (a fillet's radius, a chamfer's distance), by the
/// part of the feature it sets, as the document writes it. A new dialog of that kind opens
/// with them ([`apply`](Self::apply)); Cancel leaves them, and Edit Feature shows the
/// feature's own. Its elements aren't remembered: they're of the model they were picked on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remembered(BTreeMap<Kind, Vec<(Part, String)>>);

impl Remembered {
    /// `dialog`'s numbers kept as its kind's, if its kind remembers: each that reads as one.
    pub fn keep(&mut self, dialog: &Dialog) {
        if !dialog.kind.remembers() {
            return;
        }
        let values: Vec<(Part, String)> = dialog
            .inputs
            .iter()
            .filter_map(|input| match &input.value {
                Value::Length(text) | Value::Angle(text) => Some((input.part, number(text)?)),
                _ => None,
            })
            .collect();
        self.0.insert(dialog.kind, values);
    }

    /// A new `dialog`'s numbers set to those its kind was last OK'd with, if it was.
    pub fn apply(&self, dialog: &mut Dialog) {
        let Some(values) = self.0.get(&dialog.kind) else {
            return;
        };
        for input in &mut dialog.inputs {
            let Some((_, kept)) = values.iter().find(|(part, _)| *part == input.part) else {
                continue;
            };
            if let Value::Length(text) | Value::Angle(text) = &mut input.value {
                kept.clone_into(text);
            }
        }
    }
}

/// The part of the feature an input sets: the field `set_field` sets, and where in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// One coordinate of `position`.
    Position(usize),
    /// A number at the feature's top level, by its field.
    Number(&'static str),
    /// The sketch it sweeps.
    Sketch,
    /// The axis it turns about.
    Axis,
    /// `extent`'s form, its `type`.
    ExtentType,
    /// A number of `extent`'s, by its field.
    ExtentNumber(&'static str),
    /// `extent`'s `reversed`.
    Reversed,
    /// A list of elements' names, by its field (a fillet's `edges`), and with it the body
    /// they're of, the feature's `base`.
    Names(&'static str),
    /// A number of a chamfer's `size`, by its field.
    SizeNumber(&'static str),
    /// A body, by its field: a boolean's target, its `base`, or its tool, its `tool`.
    Body(&'static str),
}

impl Part {
    /// The field `set_field` sets for it: each coordinate sets `position` whole, and each
    /// part of the extent sets `extent` whole, as one form holds only its own values.
    pub fn field(self) -> &'static str {
        match self {
            Part::Position(_) => "position",
            Part::Number(field) => field,
            Part::Sketch => "sketch",
            Part::Axis => "axis",
            Part::ExtentType | Part::ExtentNumber(_) | Part::Reversed => "extent",
            Part::Names(field) | Part::Body(field) => field,
            Part::SizeNumber(_) => "size",
        }
    }
}

/// What an input holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A length in millimetres, as typed.
    Length(String),
    /// An angle in degrees, as typed.
    Angle(String),
    /// On or off.
    Toggle(bool),
    /// One of `options`: each the document's value and what the dialog shows of it.
    Choice {
        options: Vec<(String, String)>,
        chosen: usize,
    },
    /// A selection box: the feature picked, by its ID, if one is. The browser fills it.
    Selection(Option<FeatureId>),
    /// A selection box of elements by name, which the canvas fills.
    Names(Names),
    /// A selection box of one body, which the canvas and the browser fill.
    Body(BodyBox),
}

/// The field a box of names writes its body to, and a boolean's target box its body: the
/// feature's `base`.
pub const BASE: &str = "base";

/// The field a boolean's tool box writes its body to.
pub const TOOL: &str = "tool";

/// A selection box of one body (a boolean's Target Body or Tool Body, as Fusion's Combine
/// labels them; docs/ux/fusion.md, *Combine*), as SolidWorks' box of one entity holds it: the
/// body, by its feature's ID, the end of its chain, as the document's `base` and `tool` name
/// it; a pick replaces it. And what it last said: why it refused a pick, or what of the
/// selection it opened with it left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodyBox {
    pub body: Option<FeatureId>,
    pub said: Option<Says>,
}

/// A selection box of elements by name (a fillet's edges; section 9's *The active selection
/// box*), as SolidWorks' PropertyManager box holds them: the names it writes, in the order
/// taken, each as the document stores it (a stable name, as
/// [`Selection::storable`](crate::selection::Selection::storable) gives it); the body
/// they're of, which the feature builds on (its `base`); and what it last said: why it
/// refused an element, or what of the selection it opened with it left out. It takes one
/// kind of element, `takes`, for a feature of type `of` (`fillet`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub takes: ElementKind,
    pub of: &'static str,
    pub names: Vec<String>,
    pub body: Option<FeatureId>,
    pub said: Option<Says>,
}

/// What a box of names says under it: why it refused an element, in the error's red
/// (`renderDialog`'s `.err-text`); or a note, what of the selection it opened with it left
/// out, in the panel's subdued text (`.subdued`), since nothing is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Says {
    Refused(String),
    Noted(String),
}

impl Says {
    pub fn text(&self) -> &str {
        match self {
            Says::Refused(text) | Says::Noted(text) => text,
        }
    }

    /// The colour it's said in.
    pub fn ink(&self) -> egui::Color32 {
        match self {
            Says::Refused(_) => theme::SELECTION_BOX_SAID,
            Says::Noted(_) => theme::SELECTION_BOX_NOTE,
        }
    }
}

impl Names {
    /// An empty box of `takes`, of a feature of type `of`.
    pub fn new(takes: ElementKind, of: &'static str) -> Self {
        Names {
            takes,
            of,
            names: Vec::new(),
            body: None,
            said: None,
        }
    }

    /// Its names as JSON writes them, an array of strings: none while it holds none.
    pub fn text(&self) -> Option<String> {
        (!self.names.is_empty()).then(|| {
            let quoted: Vec<String> = self.names.iter().map(|name| quoted(name)).collect();
            format!("[{}]", quoted.join(", "))
        })
    }

    /// How many it holds, in words: "1 edge", "3 edges".
    pub fn count(&self) -> String {
        let word = if self.names.len() == 1 {
            self.takes.word()
        } else {
            self.takes.plural()
        };
        format!("{} {word}", self.names.len())
    }
}

/// What a box of names made of an element a click toggled in the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// Added to the box.
    Added,
    /// Removed from it.
    Removed,
    /// Refused, and why said: the selection lets it go again, so it shows what the box
    /// holds.
    Refused,
    /// Nothing changed: no box of names is active, or the box held it as it was.
    Ignored,
}

/// What `error` says, its message and its suggestion, as the tab says a refusal.
fn words(error: &Error) -> String {
    crate::tab::said(&Problems::from(error.clone()))
}

/// One input of a dialog: its label, the part of the feature it sets, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub label: &'static str,
    pub part: Part,
    pub value: Value,
}

/// The number `text` is, as JSON writes it (the document's numbers are read exactly from
/// their text, ARCHITECTURE.md), if it's one: what an input sends. Space around it is let
/// go; anything else isn't a number, and the input shows red.
pub fn number(text: &str) -> Option<String> {
    match json::read(text.trim().as_bytes()) {
        Ok(Node {
            value: Json::Number(number),
            ..
        }) => Some(number),
        _ => None,
    }
}

/// What the person did with a dialog this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Ok,
    Cancel,
}

/// An open dialog: the feature it makes or edits, and its inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    pub kind: Kind,
    /// The feature it edits: an existing one's, or a new one's once its add has answered.
    pub feature: Option<FeatureId>,
    /// Whether it makes its feature (a new feature's), rather than editing one there.
    pub made: bool,
    pub inputs: Vec<Input>,
    /// Each field's text, as the document holds it as far as the dialog knows: as loaded,
    /// as added, or as last sent. A field whose text differs is sent.
    sent: BTreeMap<&'static str, String>,
    /// A new one's add is sent and not yet answered.
    pub adding: bool,
    /// The selection box a pick fills (section 9, *The active selection box*): the place
    /// among the inputs of the one highlighted, if the dialog has one.
    pub active: Option<usize>,
    /// Which dialog of the tab's it is: an add's answer is this one's only if it says so.
    pub serial: u64,
    /// The names its boxes let go of (a chip's remove button) since the tab last took them:
    /// the tab lets them go from the selection too.
    unpicked: Vec<String>,
}

/// A length's input.
fn length(label: &'static str, part: Part, value: &str) -> Input {
    Input {
        label,
        part,
        value: Value::Length(value.to_owned()),
    }
}

/// The options of a choice, each the document's value and what's shown.
fn options(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(value, shown)| ((*value).to_owned(), (*shown).to_owned()))
        .collect()
}

impl Dialog {
    /// A new feature's dialog, its inputs at their defaults, sweeping `sketch` if it's one
    /// that does and one is given. Its body, a sweep's `base`, is the tab's to give when the
    /// add is sent ([`feature_node`]).
    pub fn new(kind: Kind, sketch: Option<FeatureId>, serial: u64) -> Self {
        let position = || {
            ["X", "Y", "Z"]
                .into_iter()
                .enumerate()
                .map(|(at, label)| length(label, Part::Position(at), "0"))
        };
        let extent = |forms: &[(&str, &str)]| Input {
            label: "Extent",
            part: Part::ExtentType,
            value: Value::Choice {
                options: options(forms),
                chosen: 0,
            },
        };
        let sketch_input = Input {
            label: "Sketch",
            part: Part::Sketch,
            value: Value::Selection(sketch),
        };
        let reversed = Input {
            label: "Reversed",
            part: Part::Reversed,
            value: Value::Toggle(false),
        };
        let linear = [
            ("one_side", "One side"),
            ("symmetric", "Symmetric"),
            ("two_sides", "Two sides"),
        ];
        let inputs: Vec<Input> = match kind {
            Kind::Box => position()
                .chain([
                    length("Length", Part::Number("length"), "10"),
                    length("Width", Part::Number("width"), "10"),
                    length("Height", Part::Number("height"), "10"),
                ])
                .collect(),
            Kind::Cylinder => position()
                .chain([
                    length("Radius", Part::Number("radius"), "5"),
                    length("Height", Part::Number("height"), "10"),
                ])
                .collect(),
            Kind::Sphere => position()
                .chain([length("Radius", Part::Number("radius"), "5")])
                .collect(),
            Kind::Cone => position()
                .chain([
                    length("Bottom radius", Part::Number("radius1"), "5"),
                    length("Top radius", Part::Number("radius2"), "0"),
                    length("Height", Part::Number("height"), "10"),
                ])
                .collect(),
            Kind::Torus => position()
                .chain([
                    length("Major radius", Part::Number("radius1"), "10"),
                    length("Minor radius", Part::Number("radius2"), "2"),
                ])
                .collect(),
            Kind::Pad | Kind::Pocket => {
                let mut forms = linear.to_vec();
                if kind == Kind::Pocket {
                    forms.push(("through_all", "Through all"));
                }
                let distance = if kind == Kind::Pad { "10" } else { "5" };
                vec![
                    sketch_input,
                    extent(&forms),
                    length("Length", Part::ExtentNumber("length"), distance),
                    length("Length 2", Part::ExtentNumber("length2"), distance),
                    reversed,
                ]
            }
            // Its edges, from the selection (a fillet in the core rounds edges alone: its
            // `edges` are edges' names, `references`), and its radius: the last OK'd
            // ([`Remembered`]), or at first 1 mm, Silico's own choice (SolidWorks' help gives
            // no first value, docs/ux/solidworks.md, §5).
            Kind::Fillet => vec![
                Input {
                    label: "Edges",
                    part: Part::Names("edges"),
                    value: Value::Names(Names::new(ElementKind::Edge, "fillet")),
                },
                length("Radius", Part::Number("radius"), "1"),
            ],
            // Its edges, as a fillet's, and its size, as the document's chamfer takes it:
            // `{"distance": d}`, alike along both faces (`ChamferSize`; the protocol's two
            // uneven forms need a reference face the document doesn't name yet): the last
            // OK'd, or at first 1 mm, as a fillet's radius.
            Kind::Chamfer => vec![
                Input {
                    label: "Edges",
                    part: Part::Names("edges"),
                    value: Value::Names(Names::new(ElementKind::Edge, "chamfer")),
                },
                length("Distance", Part::SizeNumber("distance"), "1"),
            ],
            // Its target and its tool, a body each, in Fusion's words (Target Body, Tool
            // Body: one tool, as Silico's booleans take one; docs/ux/fusion.md, *Combine*).
            Kind::Union | Kind::Subtract | Kind::Intersect => vec![
                Input {
                    label: "Target Body",
                    part: Part::Body(BASE),
                    value: Value::Body(BodyBox::default()),
                },
                Input {
                    label: "Tool Body",
                    part: Part::Body(TOOL),
                    value: Value::Body(BodyBox::default()),
                },
            ],
            Kind::Revolve | Kind::Groove => vec![
                sketch_input,
                Input {
                    label: "Axis",
                    part: Part::Axis,
                    value: Value::Choice {
                        options: options(&[("x_axis", "X axis"), ("y_axis", "Y axis")]),
                        chosen: 1,
                    },
                },
                extent(&[("one_side", "One side"), ("symmetric", "Symmetric")]),
                Input {
                    label: "Angle",
                    part: Part::ExtentNumber("angle"),
                    value: Value::Angle("360".to_owned()),
                },
                reversed,
            ],
        };
        let active = inputs.iter().position(|input| {
            matches!(
                input.value,
                Value::Selection(_) | Value::Names(_) | Value::Body(_)
            )
        });
        Dialog {
            kind,
            feature: None,
            made: true,
            inputs,
            sent: BTreeMap::new(),
            adding: false,
            active,
            serial,
            unpicked: Vec::new(),
        }
    }

    /// The dialog of the existing feature `id`, of `kind`, its inputs loaded from `node`,
    /// the feature as the document writes it (`text` of it): none if it isn't one a
    /// dialog of `kind` reads. Nothing is sent until an input changes.
    pub fn load(kind: Kind, id: FeatureId, node: &Node, serial: u64) -> Option<Self> {
        let mut dialog = Dialog::new(kind, None, serial);
        let extent = member(node, "extent");
        for input in &mut dialog.inputs {
            let found = match input.part {
                Part::Position(at) => match member(node, "position").map(|n| &n.value) {
                    Some(Json::Array(items)) => items.get(at),
                    _ => None,
                },
                Part::Number(field) => member(node, field),
                Part::Sketch | Part::Axis => member(node, input.part.field()),
                Part::ExtentType => extent.and_then(|extent| member(extent, "type")),
                Part::ExtentNumber(field) => extent.and_then(|extent| member(extent, field)),
                Part::Reversed => extent.and_then(|extent| member(extent, "reversed")),
                Part::Names(field) | Part::Body(field) => member(node, field),
                Part::SizeNumber(field) => {
                    member(node, "size").and_then(|size| member(size, field))
                }
            };
            let Some(found) = found else {
                // A value the form hasn't (`length2` of a one-sided pad): left at its default.
                continue;
            };
            match (&mut input.value, &found.value) {
                (Value::Length(text) | Value::Angle(text), Json::Number(number)) => {
                    number.clone_into(text);
                }
                (Value::Toggle(on), Json::Bool(value)) => *on = *value,
                (Value::Selection(picked), Json::String(text)) => {
                    *picked = Some(FeatureId::new(text)?);
                }
                (Value::Body(held), Json::String(text)) => {
                    held.body = Some(FeatureId::new(text)?);
                }
                (Value::Names(names), Json::Array(items)) => {
                    names.names = items
                        .iter()
                        .map(|item| match &item.value {
                            Json::String(name) => Some(name.clone()),
                            _ => None,
                        })
                        .collect::<Option<Vec<String>>>()?;
                    names.body = match member(node, BASE).map(|base| &base.value) {
                        Some(Json::String(base)) => Some(FeatureId::new(base)?),
                        _ => return None,
                    };
                }
                (Value::Choice { options, chosen }, Json::String(text)) => {
                    match options.iter().position(|(value, _)| value == text) {
                        Some(at) => *chosen = at,
                        // An axis that's one of the sketch's lines: offered as itself.
                        None if input.part == Part::Axis => {
                            options.push((text.clone(), text.clone()));
                            *chosen = options.len() - 1;
                        }
                        None => return None,
                    }
                }
                _ => return None,
            }
        }
        dialog.feature = Some(id);
        dialog.made = false;
        dialog.sent = dialog
            .fields()
            .into_iter()
            .filter_map(|(field, text)| Some((field, text?)))
            .collect();
        Some(dialog)
    }

    /// Whether the extent's form is one of `forms`.
    fn form_is(&self, forms: &[&str]) -> bool {
        self.inputs.iter().any(|input| match &input.value {
            Value::Choice { options, chosen } if input.part == Part::ExtentType => options
                .get(*chosen)
                .is_some_and(|(form, _)| forms.contains(&form.as_str())),
            _ => false,
        })
    }

    /// Whether `input` shows, as the extent's form has it: a second length only for two
    /// sides, a length for every form but through all, and a direction to reverse only
    /// where it changes the solid (one side, through all).
    pub fn shown(&self, input: &Input) -> bool {
        match input.part {
            Part::ExtentNumber("length") => !self.form_is(&["through_all"]),
            Part::ExtentNumber("length2") => self.form_is(&["two_sides"]),
            Part::Reversed => self.form_is(&["one_side", "through_all"]),
            _ => true,
        }
    }

    /// Whether `input` holds a value it can send: a number that reads as one, a selection
    /// made. A hidden input is never in the way.
    pub fn readable(&self, input: &Input) -> bool {
        !self.shown(input)
            || match &input.value {
                Value::Length(text) | Value::Angle(text) => number(text).is_some(),
                Value::Selection(picked) => picked.is_some(),
                Value::Names(names) => names.text().is_some() && names.body.is_some(),
                Value::Body(held) => held.body.is_some(),
                Value::Toggle(_) | Value::Choice { .. } => true,
            }
    }

    /// Each field the inputs set, and its text as the document reads it: none for a field
    /// an input of which isn't readable.
    pub fn fields(&self) -> BTreeMap<&'static str, Option<String>> {
        let mut fields: BTreeMap<&'static str, Option<String>> = BTreeMap::new();
        let mut position = [String::new(), String::new(), String::new()];
        let mut position_ok = true;
        let mut extent: Vec<(&'static str, String)> = Vec::new();
        let mut extent_ok = true;
        let mut size: Vec<(&'static str, String)> = Vec::new();
        let mut size_ok = true;
        for input in &self.inputs {
            if !self.shown(input) {
                continue;
            }
            let text = match &input.value {
                Value::Length(text) | Value::Angle(text) => number(text),
                Value::Toggle(on) => Some(on.to_string()),
                Value::Choice { options, chosen } => {
                    options.get(*chosen).map(|(value, _)| quoted(value))
                }
                Value::Selection(picked) => picked.as_ref().map(|id| quoted(id.as_str())),
                Value::Names(names) => {
                    fields.insert(BASE, names.body.as_ref().map(|body| quoted(body.as_str())));
                    names.text()
                }
                Value::Body(held) => held.body.as_ref().map(|body| quoted(body.as_str())),
            };
            match input.part {
                Part::Position(at) => match (text, position.get_mut(at)) {
                    (Some(text), Some(slot)) => *slot = text,
                    _ => position_ok = false,
                },
                Part::Number(_) | Part::Sketch | Part::Axis | Part::Names(_) | Part::Body(_) => {
                    fields.insert(input.part.field(), text);
                }
                Part::ExtentType => match text {
                    Some(text) => extent.insert(0, ("type", text)),
                    None => extent_ok = false,
                },
                Part::ExtentNumber(field) => match text {
                    Some(text) => extent.push((field, text)),
                    None => extent_ok = false,
                },
                Part::Reversed => match text {
                    Some(text) => extent.push(("reversed", text)),
                    None => extent_ok = false,
                },
                Part::SizeNumber(field) => match text {
                    Some(text) => size.push((field, text)),
                    None => size_ok = false,
                },
            }
        }
        let has = |part: fn(&Part) -> bool| self.inputs.iter().any(|input| part(&input.part));
        if has(|part| matches!(part, Part::ExtentType)) {
            let members: Vec<String> = extent
                .iter()
                .map(|(field, text)| format!("{}: {text}", quoted(field)))
                .collect();
            fields.insert(
                "extent",
                extent_ok.then(|| format!("{{{}}}", members.join(", "))),
            );
        }
        if has(|part| matches!(part, Part::SizeNumber(_))) {
            let members: Vec<String> = size
                .iter()
                .map(|(field, text)| format!("{}: {text}", quoted(field)))
                .collect();
            fields.insert(
                "size",
                size_ok.then(|| format!("{{{}}}", members.join(", "))),
            );
        }
        if has(|part| matches!(part, Part::Position(_))) {
            fields.insert(
                "position",
                position_ok.then(|| format!("[{}]", position.join(", "))),
            );
        }
        fields
    }

    /// Whether every field can be sent, and no body is in two boxes (a body can't be its
    /// own tool): OK is off until both hold.
    pub fn valid(&self) -> bool {
        self.fields().values().all(Option::is_some) && !self.one_body_twice()
    }

    /// The bodies its boxes of one body hold, in order: a boolean's target, then its tool.
    pub fn bodies(&self) -> Vec<FeatureId> {
        self.inputs
            .iter()
            .filter_map(|input| match &input.value {
                Value::Body(held) => held.body.clone(),
                _ => None,
            })
            .collect()
    }

    /// Whether two of its boxes of one body hold the same body.
    fn one_body_twice(&self) -> bool {
        let mut seen: Vec<FeatureId> = Vec::new();
        for body in self.bodies() {
            if seen.contains(&body) {
                return true;
            }
            seen.push(body);
        }
        false
    }

    /// The new feature's add, as the members of the feature the document writes (no `id`,
    /// for the core to give it one, and no `base`, the tab's to give: [`feature_node`]), if
    /// it's to be sent now: none once its feature is there, while one is under way (what
    /// changes meanwhile is sent as edits once it has answered), while the inputs aren't
    /// valid, or while they're what the last add, refused, sent.
    pub fn add(&mut self) -> Option<Vec<String>> {
        if self.feature.is_some() || self.adding || !self.valid() {
            return None;
        }
        let fields: BTreeMap<&'static str, String> = self
            .fields()
            .into_iter()
            .filter_map(|(field, text)| Some((field, text?)))
            .collect();
        if fields == self.sent {
            return None;
        }
        let mut members = vec![
            format!("\"type\": {}", quoted(self.kind.type_name())),
            format!("\"name\": {}", quoted(self.kind.title())),
        ];
        members.extend(
            fields
                .iter()
                .map(|(field, text)| format!("{}: {text}", quoted(field))),
        );
        self.sent = fields;
        self.adding = true;
        Some(members)
    }

    /// The edits the inputs ask of the feature: each field whose text differs from what
    /// the document holds as far as the dialog knows, then taken as held. None before the
    /// feature is there.
    pub fn changes(&mut self) -> Vec<(&'static str, Node)> {
        if self.feature.is_none() {
            return Vec::new();
        }
        let mut changes = Vec::new();
        for (field, text) in self.fields() {
            let Some(text) = text else { continue };
            if self.sent.get(field) == Some(&text) {
                continue;
            }
            if let Ok(node) = json::read(text.as_bytes()) {
                changes.push((field, node));
                self.sent.insert(field, text);
            }
        }
        changes
    }

    /// The active selection box filled with `sketch`, if the dialog has one.
    pub fn pick(&mut self, sketch: &FeatureId) {
        if let Some(Input {
            value: Value::Selection(picked),
            ..
        }) = self.active.and_then(|at| self.inputs.get_mut(at))
        {
            *picked = Some(sketch.clone());
        }
    }

    /// The active selection box, if it's a box of names.
    pub fn names(&self) -> Option<&Names> {
        match self.active.and_then(|at| self.inputs.get(at)) {
            Some(Input {
                value: Value::Names(names),
                ..
            }) => Some(names),
            _ => None,
        }
    }

    /// Its box of names, active or not, if it has one (a fillet's or chamfer's edges).
    pub fn names_input(&self) -> Option<&Names> {
        self.inputs.iter().find_map(|input| match &input.value {
            Value::Names(names) => Some(names),
            _ => None,
        })
    }

    /// `broken`'s name in its box of names put right (flow 6b): `stored` (a candidate's
    /// Use, the name a store writes for it on the box's base) in its place, or, for none,
    /// the name let go (Remove the edge), as the box's own remove button lets one go, so the
    /// box may empty, its OK off until an edge is picked (SolidWorks' Fillet
    /// PropertyManager, its missing references cleared). The name is found at the broken
    /// one's place in the box, if that holds it, and only otherwise by its name (two long
    /// names may share the part an error gives of them). Whether the box held the name.
    pub fn repair(&mut self, broken: &crate::repair::Broken, stored: Option<String>) -> bool {
        let Some(names) = self
            .inputs
            .iter_mut()
            .find_map(|input| match &mut input.value {
                Value::Names(names) => Some(names),
                _ => None,
            })
        else {
            return false;
        };
        let at_place = names
            .names
            .get(broken.place)
            .is_some_and(|name| broken.is(name))
            .then_some(broken.place);
        let Some(at) = at_place.or_else(|| names.names.iter().position(|name| broken.is(name)))
        else {
            return false;
        };
        names.said = None;
        match stored {
            Some(stored) => {
                if let Some(place) = names.names.get_mut(at) {
                    *place = stored;
                }
            }
            None => {
                let gone = names.names.remove(at);
                self.unpicked.push(gone);
            }
        }
        true
    }

    fn names_mut(&mut self) -> Option<&mut Names> {
        match self.active.and_then(|at| self.inputs.get_mut(at)) {
            Some(Input {
                value: Value::Names(names),
                ..
            }) => Some(names),
            _ => None,
        }
    }

    /// What the canvas picks while the dialog is open, as a selection filter: the kind of
    /// element its active box takes, if it's a box of names; every kind, if it's a box of one
    /// body, which takes the body of any element picked; else none (the canvas picks as its
    /// own filter has it).
    pub fn takes(&self) -> Option<Filter> {
        match self
            .active
            .and_then(|at| self.inputs.get(at))
            .map(|input| &input.value)
        {
            Some(Value::Names(names)) => Some(Filter {
                faces: names.takes == ElementKind::Face,
                edges: names.takes == ElementKind::Edge,
                vertices: names.takes == ElementKind::Vertex,
            }),
            Some(Value::Body(_)) => Some(Filter::default()),
            _ => None,
        }
    }

    /// The active selection box, if it's a box of one body.
    pub fn body_box(&self) -> Option<&BodyBox> {
        match self.active.and_then(|at| self.inputs.get(at)) {
            Some(Input {
                value: Value::Body(held),
                ..
            }) => Some(held),
            _ => None,
        }
    }

    fn body_box_mut(&mut self) -> Option<&mut BodyBox> {
        match self.active.and_then(|at| self.inputs.get_mut(at)) {
            Some(Input {
                value: Value::Body(held),
                ..
            }) => Some(held),
            _ => None,
        }
    }

    /// Whether its active box takes a body: the canvas's picks and the browser's body rows
    /// are its while it does.
    pub fn takes_bodies(&self) -> bool {
        self.body_box().is_some()
    }

    /// The selection box at `at` made the active one (a click on it, the prototype's
    /// `activateBox`), if it's one.
    pub fn activate(&mut self, at: usize) {
        if let Some(Input {
            value: Value::Selection(_) | Value::Names(_) | Value::Body(_),
            ..
        }) = self.inputs.get(at)
        {
            self.active = Some(at);
        }
    }

    /// The place of the first box of one body that holds none, if one doesn't.
    fn first_empty_body(&self) -> Option<usize> {
        self.inputs
            .iter()
            .position(|input| matches!(&input.value, Value::Body(held) if held.body.is_none()))
    }

    /// The bodies selected when the dialog opened (the browser's body rows, in the order
    /// selected), taken into its boxes of one body in order, the first its target and the
    /// second its tool, as SolidWorks' boxes take what's selected when its command starts;
    /// and `left_out`, what else was selected, noted under its first box. The active box is
    /// then the first left empty, if one is, else the first.
    pub fn take_bodies(&mut self, bodies: &[FeatureId], left_out: Option<String>) {
        let mut bodies = bodies.iter();
        let mut first = None;
        for (at, input) in self.inputs.iter_mut().enumerate() {
            if let Value::Body(held) = &mut input.value {
                held.body = bodies.next().cloned();
                held.said = None;
                if first.is_none() {
                    first = Some(at);
                    held.said = left_out.clone().map(Says::Noted);
                }
            }
        }
        self.active = self.first_empty_body().or(first);
    }

    /// An element a click toggled in the selection while a box of one body is active, taken
    /// as a pick of the body it's in, as SolidWorks' and Fusion's body boxes take a body from
    /// a click on it ([`set_body`](Self::set_body)), whether the click added it to the
    /// selection or let it go. Refused, and why said, if it isn't in a body the model has
    /// named yet.
    ///
    /// A pick on the body the dialog's own feature is built into (`through_own`: its own
    /// result, or a body built on it) is read against the bodies its boxes hold, as
    /// SolidWorks reads a feature's picks on the model rolled back to before it: `holds`
    /// says whether a body's solid holds the element by its name (none if it can't be read
    /// yet). The first that does is the body picked; if none does, it's the feature's own,
    /// or a later one's, and refused, said.
    pub fn took_body(
        &mut self,
        toggled: &Toggled,
        through_own: bool,
        holds: &dyn Fn(&FeatureId) -> Option<bool>,
    ) -> Taken {
        if !self.takes_bodies() {
            return Taken::Ignored;
        }
        let shown = match &toggled.stored {
            Ok(stored) => stored.to_string(),
            Err(_) => toggled.name.clone(),
        };
        let noun = self.kind.noun();
        let body = if through_own {
            let mut unread = false;
            let mut found = None;
            for input in self.bodies() {
                match holds(&input) {
                    Some(true) => {
                        found = Some(input);
                        break;
                    }
                    Some(false) => {}
                    None => unread = true,
                }
            }
            match found {
                Some(found) => found,
                None if unread => {
                    return self.refuse(format!(
                        "{shown} can't be read on the bodies this {noun} combines yet: select \
                         it again once the model has rebuilt"
                    ));
                }
                None => {
                    return self.refuse(format!(
                        "{shown} is on none of the bodies this {noun} combines: select one of \
                         them, or another body"
                    ));
                }
            }
        } else {
            match (&toggled.body, &toggled.stored) {
                (Some(body), _) => body.clone(),
                (None, Err(refusal)) => return self.refuse(words(refusal)),
                (None, Ok(_)) => {
                    return self.refuse(format!(
                        "{shown} isn't in a body the model has built: select it again once \
                         the model has rebuilt"
                    ));
                }
            }
        };
        self.set_body(body)
    }

    /// The browser's row of the feature `id` chosen while a box of one body is active: taken
    /// into it, as a pick of the body is ([`set_body`](Self::set_body)); unless it isn't a
    /// body (`is_body`: a sketch's row), or it's the dialog's own feature's body or one built
    /// on it (`through_own`), which can't be its own input: each refused, said.
    pub fn took_row(&mut self, id: &FeatureId, is_body: bool, through_own: bool) -> Taken {
        if !self.takes_bodies() {
            return Taken::Ignored;
        }
        let (a_noun, noun) = (self.kind.a_noun(), self.kind.noun());
        if !is_body {
            return self.refuse(format!("{id} isn't a body: {a_noun} takes bodies"));
        }
        if through_own {
            return self.refuse(format!(
                "{id} is built through this {noun}: select one of the bodies it combines, or \
                 another body"
            ));
        }
        self.set_body(id.clone())
    }

    /// `body` taken into the active box of one body, replacing what it held, as SolidWorks'
    /// box of one entity takes a pick; unless another box holds it (a body can't be its own
    /// tool: said, nothing taken). Then the first box left empty is the active one, if one
    /// is, as Fusion's Combine goes on from its Target Body to its Tool Bodies
    /// (docs/ux/fusion.md, *Combine*). What every box said is let go. Added if the box
    /// holds another body now; ignored if it held that one.
    fn set_body(&mut self, body: FeatureId) -> Taken {
        let Some(active) = self.active else {
            return Taken::Ignored;
        };
        let other = self
            .inputs
            .iter()
            .enumerate()
            .find_map(|(at, input)| match &input.value {
                Value::Body(held) if at != active && held.body.as_ref() == Some(&body) => {
                    Some(input.label)
                }
                _ => None,
            });
        if let Some(label) = other {
            return self.refuse(format!(
                "{body} is the {label} already: a body can't be its own tool"
            ));
        }
        for input in &mut self.inputs {
            if let Value::Body(held) = &mut input.value {
                held.said = None;
            }
        }
        let Some(held) = self.body_box_mut() else {
            return Taken::Ignored;
        };
        let changed = held.body.as_ref() != Some(&body);
        held.body = Some(body);
        if let Some(empty) = self.first_empty_body() {
            self.active = Some(empty);
        }
        if changed {
            Taken::Added
        } else {
            Taken::Ignored
        }
    }

    /// `why` said under the active box of one body: refused.
    fn refuse(&mut self, why: String) -> Taken {
        if let Some(held) = self.body_box_mut() {
            held.said = Some(Says::Refused(why));
        }
        Taken::Refused
    }

    /// The selection the dialog opened with, taken into its active box of names, if it has
    /// one: `stored`, each element's name as a store writes it
    /// ([`Selection::storable`](crate::selection::Selection::storable)), or the first
    /// refusal, said and nothing taken; `bodies`, the bodies they're in, of which there must
    /// be one, the feature's base (several: said, and nothing taken); and `left_out`, what of
    /// the selection the box doesn't take, said if nothing else is.
    pub fn take_selection(
        &mut self,
        stored: Result<Vec<Arc<Name>>, Error>,
        bodies: &[FeatureId],
        left_out: Option<String>,
    ) {
        let Some(names) = self.names_mut() else {
            return;
        };
        let stored = match stored {
            Ok(stored) => stored,
            Err(refusal) => {
                names.said = Some(Says::Refused(words(&refusal)));
                return;
            }
        };
        if let [first, second, ..] = bodies {
            let (plural, of) = (names.takes.plural(), names.of);
            let others = if bodies.len() > 2 {
                ", among others"
            } else {
                ""
            };
            names.said = Some(Says::Refused(format!(
                "The selected {plural} are on {} bodies, {first} and {second}{others}: a {of} \
                 takes one body's {plural}",
                bodies.len(),
            )));
            return;
        }
        names.names = stored.iter().map(ToString::to_string).collect();
        names.body = bodies.first().cloned();
        names.said = left_out.map(Says::Noted);
    }

    /// An element a click toggled in the selection while the dialog's box of names is
    /// active, taken into the box: one the selection let go, let go of; one it added and the
    /// box holds, let go of too (a click on a listed element removes it, whatever the
    /// selection held, as SolidWorks' box takes a click); one it added, added, unless it's
    /// of another kind than the box takes, it's made by the dialog's own feature, it can't
    /// be stored (its name unstable, or not answered yet: why, as a store refuses it), or
    /// it's on another body than the box's names (a feature builds on one); each refusal
    /// said.
    ///
    /// A pick on the body the dialog's own feature is built into (`through_own`: that
    /// feature's own body, or one built on it, as the tab's timeline chains them) is read
    /// against the box's base, as SolidWorks reads a feature's picks on the model rolled back
    /// to before it (`on_base`, [`Selection::read_on`](crate::selection::Selection::read_on)):
    /// found there, it's the base's, stored by its name there; not there (made by the
    /// feature or a later one, or of another body a boolean joined, on either side), refused,
    /// saying it isn't on the base; not read there for another reason, refused with it. The
    /// base stays the base, however many of the box's names are let go.
    ///
    /// SEAM: SolidWorks also draws the model rolled back while the dialog is open, so what
    /// can't be picked isn't shown. Here the canvas draws the published model, the feature
    /// built, and a pick of what the base hasn't is refused, until the app can show the
    /// model rolled back.
    pub fn toggled(
        &mut self,
        toggled: &Toggled,
        through_own: bool,
        on_base: &dyn Fn(&FeatureId) -> OnBase,
    ) -> Taken {
        let own = self.feature.clone();
        let Some(names) = self.names_mut() else {
            return Taken::Ignored;
        };
        // Read against the base, if it's a pick on the body the feature is built into.
        let base = names.body.clone().filter(|_| through_own);
        if !toggled.added {
            let stored = toggled.stored.as_ref().ok().map(ToString::to_string);
            let on_base = match base.as_ref().map(on_base) {
                Some(OnBase::Found(Ok(stored))) => Some(stored.to_string()),
                _ => None,
            };
            let before = names.names.len();
            names.names.retain(|name| {
                *name != toggled.name
                    && Some(name) != stored.as_ref()
                    && Some(name) != on_base.as_ref()
            });
            if names.names.len() == before {
                return Taken::Ignored;
            }
            names.said = None;
            return Taken::Removed;
        }
        let shown = match &toggled.stored {
            Ok(stored) => stored.to_string(),
            Err(_) => toggled.name.clone(),
        };
        if let Some(kind) = toggled.kind
            && kind != names.takes
        {
            names.said = Some(Says::Refused(format!(
                "{shown} is {}: a {} takes {}",
                kind.a_word(),
                names.of,
                names.takes.plural()
            )));
            return Taken::Refused;
        }
        if let (Some(own), Ok(stored)) = (&own, &toggled.stored)
            && stored.features().contains(&own)
        {
            names.said = Some(Says::Refused(format!(
                "{stored} is made by this {} itself: select one of the {} it's built on",
                names.of,
                names.takes.plural()
            )));
            return Taken::Refused;
        }
        let read = base.map(|base| {
            let read = on_base(&base);
            (base, read)
        });
        let (stored, body) = match read {
            Some((base, OnBase::Found(stored))) => (stored, Some(base)),
            Some((base, OnBase::Missing)) => {
                names.said = Some(Says::Refused(format!(
                    "{shown} isn't on {base}, which this {} is built on: select one of \
                     {base}'s {}",
                    names.of,
                    names.takes.plural()
                )));
                return Taken::Refused;
            }
            None => (toggled.stored.clone(), toggled.body.clone()),
        };
        let stored = match stored {
            Ok(stored) => stored,
            Err(refusal) => {
                names.said = Some(Says::Refused(words(&refusal)));
                return Taken::Refused;
            }
        };
        let name = stored.to_string();
        if names.names.contains(&name) {
            names.names.retain(|held| *held != name);
            names.said = None;
            return Taken::Removed;
        }
        if let Some(body) = body.filter(|body| Some(body) != own.as_ref()) {
            match &names.body {
                Some(held) if *held != body && !names.names.is_empty() => {
                    names.said = Some(Says::Refused(format!(
                        "{stored} is on {body}, and the {}'s other {} on {held}: a {} takes \
                         one body's {}",
                        names.of,
                        names.takes.plural(),
                        names.of,
                        names.takes.plural()
                    )));
                    return Taken::Refused;
                }
                _ => names.body = Some(body),
            }
        }
        names.names.push(name);
        names.said = None;
        Taken::Added
    }

    /// The names its boxes let go of since this was last asked: the tab lets them go from
    /// the selection too.
    pub fn take_unpicked(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unpicked)
    }

    /// The dialog drawn into `ui`, docked in the property panel as the prototype docks it
    /// (`renderDialog`): its title with its close button (Cancel), its inputs under INPUTS,
    /// its `preview`'s line under PREVIEW if it has one (state 3e), `said` (its feature's
    /// error, a refusal), each an error card (`errCard`, `noEdit` and `noShow`) with the
    /// name of the feature it names, if the timeline lists it, and Cancel and OK in its
    /// footer at the bottom of `ui`; `named`
    /// gives a feature's name by its ID (a selection box shows it), `refused` whether an edit
    /// of a field was refused (its input red, OK off), and `enabled` whether anything can be
    /// sent now (off while the tab closes).
    ///
    /// Left out: the Name field (the property panel's renames a feature, a dialog's would be
    /// one more way).
    ///
    /// A card of a broken reference of its feature's offers its candidates' Use and Remove
    /// the edge (its `Repair`): what they ask, and the candidate hovered, are `said`'s
    /// `out`.
    pub(crate) fn ui(
        &mut self,
        ui: &mut Ui,
        said: &mut properties::Cards,
        preview: Option<properties::Preview>,
        named: &dyn Fn(&FeatureId) -> Option<String>,
        refused: &dyn Fn(&str) -> bool,
        enabled: bool,
    ) -> Action {
        let whole = ui.max_rect();
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        let closed = properties::title(
            ui,
            Some(Icon::of_kind(self.kind.type_name())),
            self.kind.title(),
            Some(CLOSE),
        );
        let rebuild = Command::Rebuild.caps(ui.ctx()).concat();
        let (footer, cancel_at, ok_at) = properties::footer_places(whole);
        let body =
            egui::Rect::from_min_max(ui.cursor().min, egui::pos2(whole.right(), footer.top()));
        let shown: Vec<bool> = self.inputs.iter().map(|input| self.shown(input)).collect();
        let readable: Vec<bool> = self
            .inputs
            .iter()
            .map(|input| self.readable(input))
            .collect();
        let mut any_refused = false;
        let active = self.active;
        let serial = self.serial;
        let mut unpicked: Option<(usize, usize)> = None;
        let mut activated: Option<usize> = None;
        let inputs = &mut self.inputs;
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("dialog-body", self.serial))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    properties::section(ui, "Inputs");
                    for (at, input) in inputs.iter_mut().enumerate() {
                        if !shown.get(at).copied().unwrap_or(true) {
                            continue;
                        }
                        let red = red(
                            readable.get(at).copied().unwrap_or(true),
                            refused(input.part.field()),
                        );
                        any_refused |= refused(input.part.field());
                        properties::row(ui, input.label, |ui, label| {
                            if input_ui(
                                ui,
                                input,
                                label,
                                red,
                                highlighted(active, at),
                                named,
                                enabled,
                            ) {
                                activated = Some(at);
                            }
                        });
                        // A box of one body says under it what it last said.
                        if let Value::Body(BodyBox {
                            said: Some(said), ..
                        }) = &input.value
                        {
                            properties::said(ui, said.text(), said.ink());
                        }
                        // A box of names lists them under it, each with its remove button,
                        // and what it last said.
                        if let Value::Names(names) = &input.value {
                            for (place, name) in names.names.iter().enumerate() {
                                let salt = ("dialog-name", serial, at, place);
                                if properties::listed(ui, salt, icon_of(names.takes), name, enabled)
                                {
                                    unpicked = Some((at, place));
                                }
                            }
                            if let Some(said) = &names.said {
                                properties::said(ui, said.text(), said.ink());
                            }
                        }
                    }
                    if let Some(preview) = preview {
                        properties::section(ui, "Preview");
                        properties::preview(ui, preview, &rebuild);
                    }
                    let cards = &mut said.out;
                    for (place, (said, name, repair)) in said.said.iter().enumerate() {
                        // Copy as JSON, and a broken reference's repair: the dialog is the
                        // feature's edit already.
                        let out = ui
                            .push_id(("dialog-said", place), |ui| {
                                properties::repair_card(
                                    ui,
                                    said,
                                    name.as_deref(),
                                    properties::Offer::default(),
                                    repair.as_ref(),
                                )
                            })
                            .inner;
                        *cards = std::mem::take(cards).or(out);
                    }
                });
        });
        if let Some(at) = activated {
            self.activate(at);
        }
        if let Some((at, place)) = unpicked
            && let Some(Input {
                value: Value::Names(names),
                ..
            }) = self.inputs.get_mut(at)
            && place < names.names.len()
        {
            self.unpicked.push(names.names.remove(place));
            names.said = None;
        }
        properties::footer_paint(ui, footer);
        // Off while its add is unanswered: what's typed meanwhile is sent after it.
        let ok = enabled && self.valid() && !any_refused && !self.adding;
        let mut action = if closed && enabled {
            Action::Cancel
        } else {
            Action::None
        };
        // Their words and their keys' caps the registry's.
        let (cancel_word, ok_word) = (Command::Cancel.label(), Command::Ok.label());
        let cancel = properties::footer_button(
            ui,
            cancel_at,
            cancel_word,
            &Command::Cancel.keys_text(ui.ctx()),
            crate::modal::Kind::Secondary,
            enabled,
        );
        if cancel.clicked() {
            action = Action::Cancel;
        }
        let ok_cap = Command::Ok.keys_text(ui.ctx());
        if properties::footer_button(ui, ok_at, ok_word, &ok_cap, crate::modal::Kind::Primary, ok)
            .clicked()
        {
            action = Action::Ok;
        }
        action
    }
}

/// `input`'s value in its row, labelled by `label` (its key's): a field with its unit, a
/// toggle, a choice's drop-down, or a selection box (highlighted if it's the active one).
/// Whether it was clicked to be made the active box: a box of one body, of which a dialog
/// has two, is (the prototype's `activateBox`).
fn input_ui(
    ui: &mut Ui,
    input: &mut Input,
    label: egui::Id,
    red: bool,
    active: bool,
    named: &dyn Fn(&FeatureId) -> Option<String>,
    enabled: bool,
) -> bool {
    let unit = match input.value {
        Value::Length(_) => "mm",
        Value::Angle(_) => "°",
        Value::Toggle(_)
        | Value::Choice { .. }
        | Value::Selection(_)
        | Value::Names(_)
        | Value::Body(_) => "",
    };
    match &mut input.value {
        Value::Length(text) | Value::Angle(text) => {
            // The unit inside the field, at its right (`.field .unit`), the text clear of it.
            let unit_width = properties::unit_width(ui, unit);
            let salt = ("dialog-input", input.label);
            let edit = theme::field_with_room(
                ui,
                egui::TextEdit::singleline(text),
                salt,
                FieldKind::Dialog,
                red,
                unit_width + theme::FIELD_UNIT_PADDING,
            );
            let at = properties::field_rect(ui, ui.available_width());
            let field = ui
                .add_enabled_ui(enabled, |ui| ui.put(at, edit))
                .inner
                .labelled_by(label);
            properties::unit(ui, field.rect, unit);
        }
        Value::Toggle(on) => {
            ui.add_enabled(enabled, egui::Checkbox::without_text(on))
                .labelled_by(label);
        }
        Value::Choice { options, chosen } => {
            let shown: Vec<String> = options.iter().map(|(_, shown)| shown.clone()).collect();
            if let Some(place) = properties::select(ui, input.label, &shown, *chosen, enabled) {
                *chosen = place;
            }
        }
        Value::Selection(picked) => {
            let name = picked
                .as_ref()
                .map(|id| named(id).unwrap_or_else(|| id.as_str().to_owned()));
            // The active selection box, highlighted (section 9).
            properties::selection_box(
                ui,
                active,
                Icon::Sketch,
                name.as_deref(),
                "Select in the browser",
            );
        }
        Value::Names(names) => {
            // The active box, highlighted (section 9); its count, the names listed under it.
            let count = (!names.names.is_empty()).then(|| names.count());
            let empty = format!("Select {} in the canvas", names.takes.plural());
            properties::selection_box(ui, active, icon_of(names.takes), count.as_deref(), &empty);
        }
        Value::Body(held) => {
            // The body's chip, by the name the timeline gives it; or, empty, what to do. The
            // active box highlighted (section 9); a click makes it the active one.
            let name = held
                .body
                .as_ref()
                .map(|id| named(id).unwrap_or_else(|| id.as_str().to_owned()));
            let drawn = properties::selection_box(
                ui,
                active,
                Icon::Body,
                name.as_deref(),
                "Select a body in the canvas or the browser",
            );
            let clicked = ui
                .interact(
                    drawn.rect,
                    ui.id().with(("dialog-body-box", input.label)),
                    egui::Sense::click(),
                )
                .labelled_by(label);
            clicked.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, input.label)
            });
            return enabled && clicked.clicked();
        }
    }
    false
}

/// The icon a box of names shows its kind of element with: the selection filters'.
fn icon_of(kind: ElementKind) -> Icon {
    match kind {
        ElementKind::Face | ElementKind::Solid => Icon::SelFace,
        ElementKind::Edge => Icon::SelEdge,
        ElementKind::Vertex => Icon::SelVertex,
    }
}

/// The dialog's close button's label: Cancel, as its tooltip says (Cancel's own label is
/// the footer's).
pub const CLOSE: &str = "Close the dialog";

/// Whether an input shows red: its value doesn't read (unparseable, or a selection not
/// made), or the core refused its last edit.
fn red(readable: bool, refused: bool) -> bool {
    !readable || refused
}

/// Whether the input at `at` is the selection box highlighted, the `active` one.
fn highlighted(active: Option<usize>, at: usize) -> bool {
    active == Some(at)
}

/// A new feature of `kind` as `add_feature` takes it: the `members` its dialog's
/// [`Dialog::add`] gave, and, for a sweep, `base`, the body it builds on, if it has one (a
/// primitive takes none). None if they don't read as a feature's JSON, which they always do.
pub fn feature_node(kind: Kind, members: &[String], base: Option<&FeatureId>) -> Option<Node> {
    let mut members = members.to_vec();
    if let Some(base) = base.filter(|_| kind.sweeps()) {
        members.push(format!("\"base\": {}", quoted(base.as_str())));
    }
    json::read(format!("{{{}}}", members.join(", ")).as_bytes()).ok()
}

/// `text` as a JSON string.
pub(crate) fn quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            c if u32::from(c) < 0x20 => quoted.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// `node`'s member `key`, if it's an object that has one.
fn member<'a>(node: &'a Node, key: &str) -> Option<&'a Node> {
    match &node.value {
        Json::Object(members) => members
            .iter()
            .find(|member| member.key == key)
            .map(|member| &member.value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_its_json_text_and_nothing_else_is_one() {
        for (typed, want) in [
            ("10", Some("10")),
            (" 2.5 ", Some("2.5")),
            ("-1", Some("-1")),
            ("1e3", Some("1e3")),
            ("x", None),
            ("", None),
            ("1.", None),
            ("--1", None),
            ("\"1\"", None),
            ("1 2", None),
        ] {
            assert_eq!(number(typed).as_deref(), want, "{typed:?}");
        }
    }

    #[test]
    fn an_add_answered_without_a_feature_isnt_sent_again_until_an_input_changes() {
        let mut dialog = Dialog::new(Kind::Box, None, 1);
        assert!(dialog.add().is_some());
        // Under way: none.
        assert!(dialog.add().is_none());
        // Refused (the tab clears `adding`): not sent again as it was.
        dialog.adding = false;
        assert!(dialog.add().is_none());
        let length = |dialog: &mut Dialog, value: &str| {
            if let Some(Input {
                value: Value::Length(text),
                ..
            }) = dialog
                .inputs
                .iter_mut()
                .find(|input| input.label == "Length")
            {
                value.clone_into(text);
            }
        };
        // Answered: never again, whatever changes (the changes are edits then).
        dialog.feature = Some(FeatureId::new("b0000009").unwrap());
        length(&mut dialog, "12");
        assert!(dialog.add().is_none());
        dialog.feature = None;
        length(&mut dialog, "11");
        let again = feature_node(Kind::Box, &dialog.add().unwrap(), None).unwrap();
        assert_eq!(
            json::Plain::of(&again),
            json::Plain::of(
                &json::read(
                    br#"{"type": "box", "name": "Box", "height": 10, "length": 11, "position": [0, 0, 0], "width": 10}"#
                )
                .unwrap()
            )
        );
    }

    #[test]
    fn an_input_is_readable_if_its_number_reads_or_its_selection_is_made_or_its_hidden() {
        let mut pocket = Dialog::new(Kind::Pocket, None, 1);
        let at = |dialog: &Dialog, label: &str| {
            dialog
                .inputs
                .iter()
                .position(|input| input.label == label)
                .unwrap()
        };
        let readable =
            |dialog: &Dialog, label: &str| dialog.readable(&dialog.inputs[at(dialog, label)]);
        assert!(!readable(&pocket, "Sketch"));
        assert!(readable(&pocket, "Length"));
        let length = at(&pocket, "Length");
        pocket.inputs[length].value = Value::Length("5x".to_owned());
        assert!(!readable(&pocket, "Length"));
        let angle = Dialog::new(Kind::Revolve, None, 2);
        assert!(readable(&angle, "Angle"));
        // Hidden by the form (through all): never in the way.
        let form = at(&pocket, "Extent");
        pocket.inputs[form].value = Value::Choice {
            options: options(&[("through_all", "Through all")]),
            chosen: 0,
        };
        assert!(readable(&pocket, "Length"));
    }

    #[test]
    fn an_input_is_red_if_it_doesnt_read_or_was_refused_and_only_the_active_box_is_lit() {
        assert!(!red(true, false));
        assert!(red(false, false));
        assert!(red(true, true));
        assert!(red(false, true));
        assert!(highlighted(Some(2), 2));
        assert!(!highlighted(Some(2), 3));
        assert!(!highlighted(None, 0));
    }

    #[test]
    fn a_pick_fills_only_the_active_selection_box() {
        let mut pad = Dialog::new(Kind::Pad, None, 1);
        assert_eq!(pad.active, Some(0));
        // A second box, as M5.7's dialogs will have: the pick goes to the active one alone.
        pad.inputs.push(Input {
            label: "Faces",
            part: Part::Sketch,
            value: Value::Selection(None),
        });
        let sketch = FeatureId::new("s0000001").unwrap();
        pad.pick(&sketch);
        assert_eq!(pad.inputs[0].value, Value::Selection(Some(sketch.clone())));
        assert_eq!(pad.inputs[5].value, Value::Selection(None));
        pad.active = Some(5);
        let other = FeatureId::new("s0000002").unwrap();
        pad.pick(&other);
        assert_eq!(pad.inputs[0].value, Value::Selection(Some(sketch)));
        assert_eq!(pad.inputs[5].value, Value::Selection(Some(other)));
        // A primitive has none: a pick does nothing.
        let mut cube = Dialog::new(Kind::Box, None, 2);
        assert_eq!(cube.active, None);
        let before = cube.clone();
        cube.pick(&FeatureId::new("s0000001").unwrap());
        assert_eq!(cube, before);
    }

    /// Box `feature`'s edge `n`, by its name (`k7f2qx3m.box.Edge3`).
    fn edge(feature: &str, n: u32) -> Arc<Name> {
        Arc::new(Name::Element {
            feature: FeatureId::new(feature).unwrap(),
            made: silico_core::naming::TypeWord::Primitive(silico_core::naming::PrimitiveKind::Box),
            element: silico_protocol::ElementRef {
                kind: ElementKind::Edge,
                index: n,
            },
        })
    }

    /// A click's toggle of `name`, an edge, in `body`.
    fn toggle(added: bool, name: &Arc<Name>, body: &str) -> Toggled {
        Toggled {
            added,
            name: name.to_string(),
            stored: Ok(Arc::clone(name)),
            body: Some(FeatureId::new(body).unwrap()),
            kind: Some(ElementKind::Edge),
        }
    }

    impl Dialog {
        /// A toggle taken as one on a body the dialog's feature isn't built into.
        fn take(&mut self, toggled: &Toggled) -> Taken {
            self.toggled(toggled, false, &|_| panic!("read against the base"))
        }
    }

    /// A dialog whose only input is a box of edges, of a fillet, active.
    fn edges_dialog() -> Dialog {
        let mut dialog = Dialog::new(Kind::Box, None, 1);
        dialog.inputs = vec![Input {
            label: "Edges",
            part: Part::Names("edges"),
            value: Value::Names(Names::new(ElementKind::Edge, "fillet")),
        }];
        dialog.active = Some(0);
        dialog
    }

    fn field(dialog: &Dialog, name: &str) -> Option<String> {
        dialog.fields().get(name).cloned().flatten()
    }

    #[test]
    fn a_box_of_names_writes_them_as_an_array_and_their_body_as_base_once_it_holds_one() {
        let mut dialog = edges_dialog();
        assert_eq!(
            dialog.takes(),
            Some(Filter {
                faces: false,
                edges: true,
                vertices: false
            })
        );
        // Empty: neither written, and not valid.
        assert_eq!(field(&dialog, "edges"), None);
        assert_eq!(field(&dialog, "base"), None);
        assert!(!dialog.valid());
        assert_eq!(
            dialog.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")),
            Taken::Added
        );
        assert_eq!(
            dialog.take(&toggle(true, &edge("k7f2qx3m", 5), "k7f2qx3m")),
            Taken::Added
        );
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]"#)
        );
        assert_eq!(field(&dialog, "base").as_deref(), Some(r#""k7f2qx3m""#));
        assert!(dialog.valid());
        assert_eq!(dialog.names().unwrap().count(), "2 edges");
        // Clicked again while the box holds it, whatever the selection held (the browser
        // replaced it, so the click added it there): removed, as SolidWorks' box takes a
        // click on an item it lists; and clicked once more, taken again, after the rest.
        assert_eq!(
            dialog.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")),
            Taken::Removed
        );
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge5"]"#)
        );
        assert_eq!(
            dialog.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")),
            Taken::Added
        );
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge5", "k7f2qx3m.box.Edge3"]"#)
        );
        // Let go: removed; let go of one it doesn't hold: nothing.
        assert_eq!(
            dialog.take(&toggle(false, &edge("k7f2qx3m", 3), "k7f2qx3m")),
            Taken::Removed
        );
        assert_eq!(
            dialog.take(&toggle(false, &edge("k7f2qx3m", 9), "k7f2qx3m")),
            Taken::Ignored
        );
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge5"]"#)
        );
        assert_eq!(dialog.names().unwrap().count(), "1 edge");
        // A dialog with no box of names takes nothing.
        let mut cube = Dialog::new(Kind::Box, None, 2);
        assert_eq!(cube.takes(), None);
        assert_eq!(
            cube.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")),
            Taken::Ignored
        );
    }

    #[test]
    fn a_box_of_names_refuses_an_unstorable_name_another_bodys_and_its_own_features_saying_why() {
        let mut dialog = edges_dialog();
        dialog.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m"));
        let unstable = Toggled {
            stored: Err(Error::new(
                silico_core::ErrorCode::ReferenceBroken,
                "selection.1 names an edge whose name is unstable",
            )
            .with_suggestion("select another")),
            ..toggle(true, &edge("k7f2qx3m", 4), "k7f2qx3m")
        };
        assert_eq!(dialog.take(&unstable), Taken::Refused);
        let said = |dialog: &Dialog| {
            dialog
                .names()
                .unwrap()
                .said
                .as_ref()
                .map(|said| said.text().to_owned())
        };
        assert_eq!(
            said(&dialog).as_deref(),
            Some("selection.1 names an edge whose name is unstable (select another)")
        );
        // Another body's edge, while it holds one: refused, said.
        assert_eq!(
            dialog.take(&toggle(true, &edge("a91cw0de", 1), "a91cw0de")),
            Taken::Refused
        );
        assert_eq!(
            said(&dialog).as_deref(),
            Some(
                "a91cw0de.box.Edge1 is on a91cw0de, and the fillet's other edges on k7f2qx3m: \
                 a fillet takes one body's edges"
            )
        );
        // Its feature made: an edge its preview made is refused; one found in its body (the
        // preview's) is its base's, taken, the base kept.
        dialog.feature = Some(FeatureId::new("f0000001").unwrap());
        let made = Arc::new(Name::Element {
            feature: FeatureId::new("f0000001").unwrap(),
            made: silico_core::naming::TypeWord::Primitive(silico_core::naming::PrimitiveKind::Box),
            element: silico_protocol::ElementRef {
                kind: ElementKind::Edge,
                index: 1,
            },
        });
        assert_eq!(
            dialog.take(&toggle(true, &made, "f0000001")),
            Taken::Refused
        );
        assert_eq!(
            said(&dialog).as_deref(),
            Some(
                "f0000001.box.Edge1 is made by this fillet itself: select one of the edges it's \
                 built on"
            )
        );
        assert_eq!(
            dialog.take(&toggle(true, &edge("k7f2qx3m", 7), "f0000001")),
            Taken::Added
        );
        assert_eq!(said(&dialog), None);
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge7"]"#)
        );
        assert_eq!(field(&dialog, "base").as_deref(), Some(r#""k7f2qx3m""#));
        // Emptied, another body's is taken, and its body is the base.
        dialog.take(&toggle(false, &edge("k7f2qx3m", 3), "k7f2qx3m"));
        dialog.take(&toggle(false, &edge("k7f2qx3m", 7), "k7f2qx3m"));
        assert_eq!(
            dialog.take(&toggle(true, &edge("a91cw0de", 1), "a91cw0de")),
            Taken::Added
        );
        assert_eq!(field(&dialog, "base").as_deref(), Some(r#""a91cw0de""#));
    }

    #[test]
    fn a_box_of_names_takes_the_selection_it_opens_with_or_says_why_it_cant() {
        let ids = |ids: &[&str]| -> Vec<FeatureId> {
            ids.iter().map(|id| FeatureId::new(id).unwrap()).collect()
        };
        let mut dialog = edges_dialog();
        dialog.take_selection(
            Ok(vec![edge("k7f2qx3m", 3), edge("k7f2qx3m", 5)]),
            &ids(&["k7f2qx3m"]),
            Some("1 face left out: a fillet takes edges".to_owned()),
        );
        let names = dialog.names().unwrap();
        assert_eq!(names.names, ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
        assert_eq!(names.body, Some(FeatureId::new("k7f2qx3m").unwrap()));
        // A note, not a refusal: said in the panel's subdued text, not the error's red.
        assert_eq!(
            names.said,
            Some(Says::Noted(
                "1 face left out: a fillet takes edges".to_owned()
            ))
        );
        assert_eq!(names.said.as_ref().unwrap().ink(), theme::TEXT_SUBDUED);
        assert_ne!(theme::TEXT_SUBDUED, theme::ERROR_TEXT);
        // Refused: nothing taken, why said.
        let mut dialog = edges_dialog();
        dialog.take_selection(
            Err(Error::new(
                silico_core::ErrorCode::Conflict,
                "Edge3 isn't answered yet",
            )),
            &ids(&["k7f2qx3m"]),
            None,
        );
        assert!(dialog.names().unwrap().names.is_empty());
        let said = dialog.names().unwrap().said.as_ref().unwrap();
        assert_eq!(said, &Says::Refused("Edge3 isn't answered yet".to_owned()));
        assert_eq!(said.ink(), theme::ERROR_TEXT);
        assert!(!dialog.valid());
        // On three bodies: nothing taken.
        let mut dialog = edges_dialog();
        dialog.take_selection(
            Ok(vec![
                edge("k7f2qx3m", 3),
                edge("a91cw0de", 1),
                edge("b0000001", 1),
            ]),
            &ids(&["k7f2qx3m", "a91cw0de", "b0000001"]),
            None,
        );
        assert!(dialog.names().unwrap().names.is_empty());
        assert_eq!(
            dialog.names().unwrap().said,
            Some(Says::Refused(
                "The selected edges are on 3 bodies, k7f2qx3m and a91cw0de, among others: a \
                 fillet takes one body's edges"
                    .to_owned()
            ))
        );
    }

    #[test]
    fn a_fillets_dialog_loads_its_edges_base_and_radius_and_refuses_edges_it_cant_read() {
        let read = |text: &str| json::read(text.as_bytes()).unwrap();
        let id = FeatureId::new("f0000001").unwrap();
        let fillet = read(
            r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"], "radius": 2.5}"#,
        );
        let dialog = Dialog::load(Kind::Fillet, id.clone(), &fillet, 1).unwrap();
        let names = dialog.names().unwrap();
        assert_eq!(names.names, ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]);
        assert_eq!(names.body, Some(FeatureId::new("k7f2qx3m").unwrap()));
        assert_eq!(dialog.inputs[1].value, Value::Length("2.5".to_owned()));
        // Loaded as the document holds it: nothing to send.
        let mut loaded = dialog.clone();
        assert!(loaded.changes().is_empty());
        // An edge that isn't a string, or a base that isn't an ID: not a fillet it reads.
        for bad in [
            r#"{"type": "fillet", "base": "k7f2qx3m", "edges": [3], "radius": 1}"#,
            r#"{"type": "fillet", "base": 7, "edges": ["k7f2qx3m.box.Edge3"], "radius": 1}"#,
        ] {
            assert!(
                Dialog::load(Kind::Fillet, id.clone(), &read(bad), 1).is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_repair_acts_on_the_broken_names_place_where_two_long_names_share_what_its_error_gives() {
        // Two names of more than 200 characters, alike in their first 200: the error gives
        // the second's cut short there, so its words fit either. The second's place is
        // mended, the first left as it is.
        let read = |text: &str| json::read(text.as_bytes()).unwrap();
        let prefix = format!("k7f2qx3m.box.Edge5{}", "x".repeat(200));
        let (first, second) = (format!("{prefix}a"), format!("{prefix}b"));
        let fillet = read(&format!(
            r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["{first}", "{second}"], "radius": 2.5}}"#
        ));
        let broken = |place: usize| crate::repair::Broken {
            feature: FeatureId::new("f0000001").unwrap(),
            place,
            name: format!("{}…", prefix.chars().take(200).collect::<String>()),
            candidates: vec!["k7f2qx3m.box.Edge4".to_owned()],
            read: silico_core::command::Revision::OPENED,
        };
        let id = FeatureId::new("f0000001").unwrap();
        let mut used = Dialog::load(Kind::Fillet, id.clone(), &fillet, 1).unwrap();
        assert!(used.repair(&broken(1), Some("k7f2qx3m.box.Edge4".to_owned())));
        assert_eq!(
            used.names().unwrap().names,
            [first.clone(), "k7f2qx3m.box.Edge4".to_owned()]
        );
        let mut removed = Dialog::load(Kind::Fillet, id.clone(), &fillet, 1).unwrap();
        assert!(removed.repair(&broken(1), None));
        assert_eq!(removed.names().unwrap().names, std::slice::from_ref(&first));
        assert_eq!(removed.take_unpicked(), std::slice::from_ref(&second));
        // Its place holding another name (an edit since moved it): found by its name.
        let mut moved = Dialog::load(Kind::Fillet, id, &fillet, 1).unwrap();
        assert!(moved.repair(&broken(5), Some("k7f2qx3m.box.Edge4".to_owned())));
        assert_eq!(
            moved.names().unwrap().names,
            ["k7f2qx3m.box.Edge4".to_owned(), second]
        );
    }

    #[test]
    fn a_new_fillet_adds_its_type_name_base_edges_and_radius_once_it_has_edges() {
        let mut fillet = Dialog::new(Kind::Fillet, None, 1);
        // No edges: nothing to add.
        assert!(fillet.add().is_none());
        fillet.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m"));
        let members = fillet.add().unwrap();
        let node = feature_node(Kind::Fillet, &members, None).unwrap();
        assert_eq!(
            json::Plain::of(&node),
            json::Plain::of(
                &json::read(
                    br#"{"type": "fillet", "name": "Fillet", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "radius": 1}"#
                )
                .unwrap()
            )
        );
    }

    #[test]
    fn a_chamfers_dialog_writes_its_distance_as_its_size_and_loads_it_back() {
        let mut chamfer = Dialog::new(Kind::Chamfer, None, 1);
        chamfer.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m"));
        let members = chamfer.add().unwrap();
        let node = feature_node(Kind::Chamfer, &members, None).unwrap();
        let written = br#"{"type": "chamfer", "name": "Chamfer", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 1}}"#;
        assert_eq!(
            json::Plain::of(&node),
            json::Plain::of(&json::read(written).unwrap())
        );
        // Read back: its distance in its input, nothing to send.
        let read = json::read(
            br#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": 0.5}}"#,
        )
        .unwrap();
        let id = FeatureId::new("c0000001").unwrap();
        let mut loaded = Dialog::load(Kind::Chamfer, id.clone(), &read, 2).unwrap();
        assert_eq!(loaded.inputs[1].value, Value::Length("0.5".to_owned()));
        assert!(loaded.changes().is_empty());
        // A distance typed is an edit of `size` whole.
        loaded.inputs[1].value = Value::Length("2".to_owned());
        let changes = loaded.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "size");
        assert_eq!(
            json::Plain::of(&changes[0].1),
            json::Plain::of(&json::read(br#"{"distance": 2}"#).unwrap())
        );
        // A size it doesn't read: not a chamfer it loads.
        let uneven = json::read(
            br#"{"type": "chamfer", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "size": {"distance": "a"}}"#,
        )
        .unwrap();
        assert!(Dialog::load(Kind::Chamfer, id, &uneven, 3).is_none());
    }

    #[test]
    fn a_box_of_edges_refuses_a_face_whatever_the_canvas_picked() {
        // The canvas picks only what the box takes; a face that reached it all the same is
        // refused, said, and nothing written.
        let mut dialog = edges_dialog();
        let face = Toggled {
            kind: Some(ElementKind::Face),
            ..toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")
        };
        let face = Toggled {
            name: "k7f2qx3m.box.Face3".to_owned(),
            stored: Ok(Arc::new(Name::Element {
                feature: FeatureId::new("k7f2qx3m").unwrap(),
                made: silico_core::naming::TypeWord::Primitive(
                    silico_core::naming::PrimitiveKind::Box,
                ),
                element: silico_protocol::ElementRef {
                    kind: ElementKind::Face,
                    index: 3,
                },
            })),
            ..face
        };
        assert_eq!(dialog.take(&face), Taken::Refused);
        assert_eq!(
            dialog.names().unwrap().said,
            Some(Says::Refused(
                "k7f2qx3m.box.Face3 is a face: a fillet takes edges".to_owned()
            ))
        );
        assert!(dialog.names().unwrap().names.is_empty());
        assert_eq!(dialog.names().unwrap().body, None);
        assert_eq!(field(&dialog, "edges"), None);
        // An edge is taken.
        assert_eq!(
            dialog.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")),
            Taken::Added
        );
    }

    #[test]
    fn a_box_of_edges_refuses_a_vertex_whatever_the_canvas_picked() {
        // As a face is: the canvas picks only what the box takes; a vertex that reached it
        // all the same is refused, said, and nothing written, in a fillet's box and a
        // chamfer's.
        let vertex = Toggled {
            name: "k7f2qx3m.box.Vertex2".to_owned(),
            stored: Ok(Arc::new(Name::Element {
                feature: FeatureId::new("k7f2qx3m").unwrap(),
                made: silico_core::naming::TypeWord::Primitive(
                    silico_core::naming::PrimitiveKind::Box,
                ),
                element: silico_protocol::ElementRef {
                    kind: ElementKind::Vertex,
                    index: 2,
                },
            })),
            kind: Some(ElementKind::Vertex),
            ..toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")
        };
        let mut fillet = edges_dialog();
        let mut chamfer = Dialog::new(Kind::Chamfer, None, 1);
        for (dialog, of) in [(&mut fillet, "fillet"), (&mut chamfer, "chamfer")] {
            assert_eq!(dialog.take(&vertex), Taken::Refused, "{of}");
            assert_eq!(
                dialog.names().unwrap().said,
                Some(Says::Refused(format!(
                    "k7f2qx3m.box.Vertex2 is a vertex: a {of} takes edges"
                ))),
                "{of}"
            );
            assert!(dialog.names().unwrap().names.is_empty(), "{of}");
            assert_eq!(dialog.names().unwrap().body, None, "{of}");
            assert_eq!(field(dialog, "edges"), None, "{of}");
            // An edge is taken.
            assert_eq!(
                dialog.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")),
                Taken::Added,
                "{of}"
            );
        }
    }

    #[test]
    fn a_pick_on_the_body_its_feature_is_built_into_is_read_against_its_base() {
        // A fillet f0000001 on k7f2qx3m, open under Edit Feature, a later feature's body
        // r0000001 built through it. A pick there is read against the base: found, it's the
        // base's, by its name there, and the base stays; not there, it's refused, said; the
        // fillet's own edge is refused as its own, before any read.
        let fillet = json::read(
            br#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3"], "radius": 1}"#,
        )
        .unwrap();
        let id = FeatureId::new("f0000001").unwrap();
        let mut dialog = Dialog::load(Kind::Fillet, id, &fillet, 1).unwrap();
        let base = FeatureId::new("k7f2qx3m").unwrap();
        let read = std::cell::RefCell::new(Vec::new());
        let found = |at: &FeatureId| {
            read.borrow_mut().push(at.clone());
            OnBase::Found(Ok(edge("k7f2qx3m", 5)))
        };
        let on_later = toggle(true, &edge("k7f2qx3m", 5), "r0000001");
        assert_eq!(dialog.toggled(&on_later, true, &found), Taken::Added);
        assert_eq!(*read.borrow(), std::slice::from_ref(&base));
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"]"#)
        );
        assert_eq!(field(&dialog, "base").as_deref(), Some(r#""k7f2qx3m""#));
        // Clicked again: the box holds it by its name on the base, so it's let go.
        let let_go = toggle(false, &edge("k7f2qx3m", 5), "r0000001");
        assert_eq!(dialog.toggled(&let_go, true, &found), Taken::Removed);
        // An edge the later feature added: not on the base, refused.
        let added = toggle(true, &edge("r0000001", 2), "r0000001");
        assert_eq!(
            dialog.toggled(&added, true, &|_| OnBase::Missing),
            Taken::Refused
        );
        assert_eq!(
            dialog.names().unwrap().said,
            Some(Says::Refused(
                "r0000001.box.Edge2 isn't on k7f2qx3m, which this fillet is built on: select \
                 one of k7f2qx3m's edges"
                    .to_owned()
            ))
        );
        // Unstable on the base: the base's refusal said.
        let unstable = OnBase::Found(Err(Error::new(
            silico_core::ErrorCode::ReferenceBroken,
            "unstable there",
        )));
        assert_eq!(
            dialog.toggled(&on_later, true, &|_| unstable.clone()),
            Taken::Refused
        );
        assert_eq!(
            dialog.names().unwrap().said,
            Some(Says::Refused("unstable there".to_owned()))
        );
        // The fillet's own edge: its own, said so, no read made.
        let own = toggle(true, &edge("f0000001", 1), "r0000001");
        assert_eq!(
            dialog.toggled(&own, true, &|_| panic!("read against the base")),
            Taken::Refused
        );
        // Every name let go, then one picked on the later body: the base stays.
        let first = toggle(false, &edge("k7f2qx3m", 3), "r0000001");
        assert_eq!(
            dialog.toggled(&first, true, &|_| OnBase::Found(Ok(edge("k7f2qx3m", 3)))),
            Taken::Removed
        );
        assert!(dialog.names().unwrap().names.is_empty());
        assert_eq!(dialog.toggled(&on_later, true, &found), Taken::Added);
        assert_eq!(field(&dialog, "base").as_deref(), Some(r#""k7f2qx3m""#));
        assert_eq!(
            field(&dialog, "edges").as_deref(),
            Some(r#"["k7f2qx3m.box.Edge5"]"#)
        );
        // Not through the feature (another body, the box emptied): that body's, as before.
        let mut other = dialog.clone();
        other.toggled(&let_go, true, &found);
        let elsewhere = toggle(true, &edge("a91cw0de", 1), "a91cw0de");
        assert_eq!(
            other.toggled(&elsewhere, false, &|_| panic!("read against the base")),
            Taken::Added
        );
        assert_eq!(field(&other, "base").as_deref(), Some(r#""a91cw0de""#));
    }

    #[test]
    fn a_let_go_drops_the_boxs_name_by_whatever_name_reaches_it_and_an_emptied_box_cant_be_sent() {
        // A fillet f0000001 on k7f2qx3m's Edge3 and Edge5, open under Edit Feature, a later
        // feature's body r0000001 built through it.
        let fillet = json::read(
            br#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge3", "k7f2qx3m.box.Edge5"], "radius": 1}"#,
        )
        .unwrap();
        let id = FeatureId::new("f0000001").unwrap();
        let mut dialog = Dialog::load(Kind::Fillet, id, &fillet, 1).unwrap();
        let base = FeatureId::new("k7f2qx3m").unwrap();
        // Let go on r0000001 by the name the later body gives it, r0000001.box.Edge9, which
        // the box doesn't hold: read against the base, it's Edge5 there, so Edge5 is let go.
        let later = toggle(false, &edge("r0000001", 9), "r0000001");
        assert_eq!(
            dialog.toggled(&later, true, &|_| OnBase::Found(Ok(edge("k7f2qx3m", 5)))),
            Taken::Removed
        );
        assert_eq!(dialog.names().unwrap().names, ["k7f2qx3m.box.Edge3"]);
        // Let go by the name the box holds, though nothing can be stored of it now (its
        // body's elements not named yet): let go by that name.
        let waiting = Toggled {
            added: false,
            name: "k7f2qx3m.box.Edge3".to_owned(),
            stored: Err(Error::new(
                silico_core::ErrorCode::Conflict,
                "k7f2qx3m.box.Edge3 isn't answered yet",
            )),
            body: None,
            kind: None,
        };
        assert_eq!(
            dialog.toggled(&waiting, false, &|_| panic!("read against the base")),
            Taken::Removed
        );
        // Emptied: the base stays, but the box holds nothing to send, so it isn't readable,
        // and the dialog can't be sent.
        let names = dialog.names().unwrap();
        assert!(names.names.is_empty());
        assert_eq!(names.body, Some(base));
        let at = dialog
            .inputs
            .iter()
            .position(|input| matches!(input.value, Value::Names(_)))
            .unwrap();
        assert!(!dialog.readable(&dialog.inputs[at]));
        assert!(!dialog.valid());
        assert_eq!(field(&dialog, "edges"), None);
    }

    #[test]
    fn a_dressings_numbers_are_remembered_from_its_last_ok_and_others_are_not() {
        let mut remembered = Remembered::default();
        // Nothing OK'd yet: the first values, 1 mm.
        let mut fillet = Dialog::new(Kind::Fillet, None, 1);
        remembered.apply(&mut fillet);
        assert_eq!(fillet.inputs[1].value, Value::Length("1".to_owned()));
        // A fillet OK'd at 2.5 (typed with space about it), its edges held: a new fillet
        // opens at 2.5, with no edges; a chamfer at its own first value.
        fillet.inputs[1].value = Value::Length(" 2.5 ".to_owned());
        fillet.take(&toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m"));
        remembered.keep(&fillet);
        let mut again = Dialog::new(Kind::Fillet, None, 2);
        remembered.apply(&mut again);
        assert_eq!(again.inputs[1].value, Value::Length("2.5".to_owned()));
        assert!(again.names().unwrap().names.is_empty());
        let mut chamfer = Dialog::new(Kind::Chamfer, None, 3);
        remembered.apply(&mut chamfer);
        assert_eq!(chamfer.inputs[1].value, Value::Length("1".to_owned()));
        // A chamfer OK'd at 0.5: its own, the fillet's kept.
        chamfer.inputs[1].value = Value::Length("0.5".to_owned());
        remembered.keep(&chamfer);
        let mut chamfer = Dialog::new(Kind::Chamfer, None, 4);
        remembered.apply(&mut chamfer);
        assert_eq!(chamfer.inputs[1].value, Value::Length("0.5".to_owned()));
        let mut fillet = Dialog::new(Kind::Fillet, None, 5);
        remembered.apply(&mut fillet);
        assert_eq!(fillet.inputs[1].value, Value::Length("2.5".to_owned()));
        // A box doesn't remember.
        let mut cube = Dialog::new(Kind::Box, None, 7);
        cube.inputs[3].value = Value::Length("25".to_owned());
        remembered.keep(&cube);
        let mut cube = Dialog::new(Kind::Box, None, 8);
        remembered.apply(&mut cube);
        assert_eq!(cube.inputs[3].value, Value::Length("10".to_owned()));
    }

    #[test]
    fn a_sweep_with_no_sketch_isnt_valid_and_a_primitive_takes_no_base() {
        let base = FeatureId::new("b0000001").unwrap();
        let mut pad = Dialog::new(Kind::Pad, None, 1);
        assert!(!pad.valid());
        assert!(pad.add().is_none());
        pad.pick(&FeatureId::new("s0000001").unwrap());
        assert!(pad.valid());
        let members = pad.add().unwrap();
        let swept = feature_node(Kind::Pad, &members, Some(&base)).unwrap();
        assert!(format!("{:?}", json::Plain::of(&swept)).contains("b0000001"));
        let mut cube = Dialog::new(Kind::Box, None, 2);
        let members = cube.add().unwrap();
        let added = feature_node(Kind::Box, &members, Some(&base)).unwrap();
        assert!(!format!("{:?}", json::Plain::of(&added)).contains("b0000001"));
    }

    // ---- The booleans' boxes of one body ----

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    /// The bodies a dialog's boxes of one body hold, in order, by ID: none for an empty one.
    fn held(dialog: &Dialog) -> Vec<Option<&str>> {
        dialog
            .inputs
            .iter()
            .filter_map(|input| match &input.value {
                Value::Body(held) => Some(held.body.as_ref().map(FeatureId::as_str)),
                _ => None,
            })
            .collect()
    }

    /// What the box of one body labelled `label` says under it, if anything.
    fn said_under(dialog: &Dialog, label: &str) -> Option<Says> {
        dialog.inputs.iter().find_map(|input| match &input.value {
            Value::Body(held) if input.label == label => held.said.clone(),
            _ => None,
        })
    }

    fn refused(text: &str) -> Option<Says> {
        Some(Says::Refused(text.to_owned()))
    }

    #[test]
    fn each_kind_is_called_in_a_sentence_with_its_article_and_without() {
        let words: Vec<(&str, &str)> = Kind::ALL
            .into_iter()
            .map(|kind| (kind.a_noun(), kind.noun()))
            .collect();
        assert_eq!(
            words,
            [
                ("a box", "box"),
                ("a cylinder", "cylinder"),
                ("a sphere", "sphere"),
                ("a cone", "cone"),
                ("a torus", "torus"),
                ("a pad", "pad"),
                ("a pocket", "pocket"),
                ("a revolve", "revolve"),
                ("a groove", "groove"),
                ("a fillet", "fillet"),
                ("a chamfer", "chamfer"),
                ("a union", "union"),
                ("a subtraction", "subtraction"),
                ("an intersection", "intersection"),
            ]
        );
    }

    #[test]
    fn each_booleans_dialog_has_a_target_and_a_tool_body_and_adds_them_as_its_base_and_tool() {
        // Union, Subtract, and Intersect (the prototype's commands) make the document's fuse,
        // cut, and common; their boxes are Fusion's Target Body and Tool Body, the second
        // singular as the booleans take one tool (docs/ux/fusion.md, *Combine*).
        for (kind, type_name, title, a_noun) in [
            (Kind::Union, "fuse", "Union", "a union"),
            (Kind::Subtract, "cut", "Subtract", "a subtraction"),
            (Kind::Intersect, "common", "Intersect", "an intersection"),
        ] {
            assert_eq!(kind.type_name(), type_name);
            assert_eq!(kind.title(), title);
            assert_eq!(kind.a_noun(), a_noun);
            assert_eq!(Kind::of(type_name), Some(kind));
            assert!(kind.combines() && kind.modifies(), "{title}");
            assert!(!kind.dresses() && !kind.sweeps() && !kind.cuts(), "{title}");
            assert!(!kind.remembers(), "{title}");
            let mut dialog = Dialog::new(kind, None, 1);
            let labels: Vec<&str> = dialog.inputs.iter().map(|input| input.label).collect();
            assert_eq!(labels, ["Target Body", "Tool Body"], "{title}");
            // The target's box active; the canvas picks every kind of element, each taking
            // its body.
            assert_eq!(dialog.active, Some(0), "{title}");
            assert!(dialog.takes_bodies(), "{title}");
            assert_eq!(
                dialog.takes(),
                Some(Filter {
                    faces: true,
                    edges: true,
                    vertices: true
                }),
                "{title}"
            );
            // Nothing to add until both boxes hold a body.
            assert!(!dialog.valid(), "{title}");
            assert!(dialog.add().is_none(), "{title}");
            assert_eq!(
                dialog.took_row(&fid("k7f2qx3m"), true, false),
                Taken::Added,
                "{title}"
            );
            // The tool's box, empty, is the active one now.
            assert_eq!(dialog.active, Some(1), "{title}");
            assert!(dialog.add().is_none(), "{title}");
            assert_eq!(
                dialog.took_row(&fid("a91cw0de"), true, false),
                Taken::Added,
                "{title}"
            );
            assert_eq!(dialog.active, Some(1), "{title}");
            assert!(dialog.valid(), "{title}");
            let node = feature_node(kind, &dialog.add().unwrap(), None).unwrap();
            let want = format!(
                r#"{{"type": "{type_name}", "name": "{title}", "base": "k7f2qx3m", "tool": "a91cw0de"}}"#
            );
            assert_eq!(
                json::Plain::of(&node),
                json::Plain::of(&json::read(want.as_bytes()).unwrap()),
                "{title}"
            );
        }
    }

    #[test]
    fn a_second_pick_replaces_a_boxs_body_and_one_body_in_both_boxes_is_refused() {
        let mut dialog = Dialog::new(Kind::Subtract, None, 1);
        dialog.took_row(&fid("aaaaaaaa"), true, false);
        dialog.took_row(&fid("bbbbbbbb"), true, false);
        assert_eq!(held(&dialog), [Some("aaaaaaaa"), Some("bbbbbbbb")]);
        // A second pick in the tool's box replaces its body, as SolidWorks' box of one entity
        // takes one; the box stays the active one; the same pick again changes nothing.
        assert_eq!(dialog.took_row(&fid("cccccccc"), true, false), Taken::Added);
        assert_eq!(held(&dialog), [Some("aaaaaaaa"), Some("cccccccc")]);
        assert_eq!(dialog.active, Some(1));
        assert_eq!(
            dialog.took_row(&fid("cccccccc"), true, false),
            Taken::Ignored
        );
        // The target's body in the tool's box: refused, said, nothing taken.
        assert_eq!(
            dialog.took_row(&fid("aaaaaaaa"), true, false),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Tool Body"),
            refused("aaaaaaaa is the Target Body already: a body can't be its own tool")
        );
        assert_eq!(held(&dialog), [Some("aaaaaaaa"), Some("cccccccc")]);
        assert!(dialog.valid());
        // The target's box made the active one (a click on it): a pick replaces the target,
        // the box stays active (none is empty), and the refusal is let go.
        dialog.activate(0);
        assert_eq!(dialog.active, Some(0));
        assert_eq!(dialog.took_row(&fid("dddddddd"), true, false), Taken::Added);
        assert_eq!(held(&dialog), [Some("dddddddd"), Some("cccccccc")]);
        assert_eq!(dialog.active, Some(0));
        assert_eq!(said_under(&dialog, "Tool Body"), None);
        assert_eq!(
            dialog.took_row(&fid("cccccccc"), true, false),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Target Body"),
            refused("cccccccc is the Tool Body already: a body can't be its own tool")
        );
        // An input that's no selection box isn't made active.
        let mut cube = Dialog::new(Kind::Box, None, 2);
        cube.activate(3);
        assert_eq!(cube.active, None);
        // One body in both boxes, as a document the gate refuses has it: never valid, so
        // OK stays off.
        let both = json::read(
            br#"{"id": "n0000001", "type": "fuse", "v": 1, "name": "U", "base": "aaaaaaaa", "tool": "aaaaaaaa"}"#,
        )
        .unwrap();
        let both = Dialog::load(Kind::Union, fid("n0000001"), &both, 3).unwrap();
        assert_eq!(held(&both), [Some("aaaaaaaa"), Some("aaaaaaaa")]);
        assert!(!both.valid());
    }

    #[test]
    fn a_click_on_any_element_takes_its_body_and_one_in_no_built_body_is_refused() {
        let mut dialog = Dialog::new(Kind::Union, None, 1);
        // A face of k7f2qx3m, added to the selection by the click: its body, the target.
        let face = Toggled {
            kind: Some(ElementKind::Face),
            ..toggle(true, &edge("k7f2qx3m", 3), "k7f2qx3m")
        };
        assert_eq!(
            dialog.took_body(&face, false, &|_| panic!("read")),
            Taken::Added
        );
        assert_eq!(held(&dialog), [Some("k7f2qx3m"), None]);
        assert_eq!(dialog.active, Some(1));
        // A vertex of a91cw0de the click let go of in the selection: a click all the same,
        // its body taken.
        let vertex = Toggled {
            kind: Some(ElementKind::Vertex),
            ..toggle(false, &edge("a91cw0de", 2), "a91cw0de")
        };
        assert_eq!(
            dialog.took_body(&vertex, false, &|_| panic!("read")),
            Taken::Added
        );
        assert_eq!(held(&dialog), [Some("k7f2qx3m"), Some("a91cw0de")]);
        // One not answered yet, in no body known: refused in the store's words.
        let waiting = Toggled {
            added: true,
            name: "k7f2qx3m.box.Edge9".to_owned(),
            stored: Err(Error::new(
                silico_core::ErrorCode::Conflict,
                "k7f2qx3m.box.Edge9 isn't answered yet: its body's elements aren't named yet",
            )
            .with_suggestion("select it again once the model has rebuilt")),
            body: None,
            kind: None,
        };
        assert_eq!(
            dialog.took_body(&waiting, false, &|_| panic!("read")),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Tool Body"),
            refused(
                "k7f2qx3m.box.Edge9 isn't answered yet: its body's elements aren't named yet \
                 (select it again once the model has rebuilt)"
            )
        );
        assert_eq!(held(&dialog), [Some("k7f2qx3m"), Some("a91cw0de")]);
        // One whose name stores but whose body no model gave: refused too.
        let bodiless = Toggled {
            body: None,
            ..toggle(true, &edge("k7f2qx3m", 4), "k7f2qx3m")
        };
        assert_eq!(
            dialog.took_body(&bodiless, false, &|_| panic!("read")),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Tool Body"),
            refused(
                "k7f2qx3m.box.Edge4 isn't in a body the model has built: select it again once \
                 the model has rebuilt"
            )
        );
        assert_eq!(held(&dialog), [Some("k7f2qx3m"), Some("a91cw0de")]);
        // A dialog whose active box is no body's takes none.
        let mut fillet = Dialog::new(Kind::Fillet, None, 2);
        let before = fillet.clone();
        assert_eq!(
            fillet.took_body(&face, false, &|_| panic!("read")),
            Taken::Ignored
        );
        assert_eq!(
            fillet.took_row(&fid("k7f2qx3m"), true, false),
            Taken::Ignored
        );
        assert_eq!(fillet, before);
        assert!(!fillet.takes_bodies());
        // And a box of one body takes no edge into a box of names.
        let mut union = Dialog::new(Kind::Union, None, 3);
        assert_eq!(union.take(&face), Taken::Ignored);
        assert_eq!(held(&union), [None, None]);
    }

    #[test]
    fn a_pick_on_the_booleans_own_body_takes_the_body_its_element_came_from() {
        // A union n0000001 of aaaaaaaa and bbbbbbbb, open under Edit Feature: the canvas draws
        // its preview, its own body, which holds both bodies' elements. A pick there is read
        // against the bodies its boxes hold, the target first, as SolidWorks reads a pick on
        // the model rolled back to before the feature.
        let node = json::read(
            br#"{"id": "n0000001", "type": "fuse", "v": 1, "name": "Joined", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#,
        )
        .unwrap();
        let mut dialog = Dialog::load(Kind::Union, fid("n0000001"), &node, 1).unwrap();
        assert_eq!(held(&dialog), [Some("aaaaaaaa"), Some("bbbbbbbb")]);
        // Both full: the target's box is the active one.
        assert_eq!(dialog.active, Some(0));
        let asked = std::cell::RefCell::new(Vec::new());
        let in_only = |body: &'static str| {
            let asked = &asked;
            move |at: &FeatureId| {
                asked.borrow_mut().push(at.to_string());
                Some(at.as_str() == body)
            }
        };
        // The target's own element: the target, as held (nothing changes), the tool unread.
        let picked = toggle(true, &edge("aaaaaaaa", 2), "n0000001");
        assert_eq!(
            dialog.took_body(&picked, true, &in_only("aaaaaaaa")),
            Taken::Ignored
        );
        assert_eq!(*asked.borrow(), ["aaaaaaaa"]);
        // The tool's element in the target's box: the tool's body, which it holds already.
        asked.borrow_mut().clear();
        let picked = toggle(true, &edge("bbbbbbbb", 4), "n0000001");
        assert_eq!(
            dialog.took_body(&picked, true, &in_only("bbbbbbbb")),
            Taken::Refused
        );
        assert_eq!(*asked.borrow(), ["aaaaaaaa", "bbbbbbbb"]);
        assert_eq!(
            said_under(&dialog, "Target Body"),
            refused("bbbbbbbb is the Tool Body already: a body can't be its own tool")
        );
        // In neither (the union's own, or a later feature's): refused.
        let own = toggle(true, &edge("n0000001", 1), "n0000001");
        assert_eq!(
            dialog.took_body(&own, true, &|_| Some(false)),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Target Body"),
            refused(
                "n0000001.box.Edge1 is on none of the bodies this union combines: select one \
                 of them, or another body"
            )
        );
        // Neither read yet: said so.
        assert_eq!(
            dialog.took_body(&own, true, &|at| (at.as_str() == "bbbbbbbb")
                .then_some(false)),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Target Body"),
            refused(
                "n0000001.box.Edge1 can't be read on the bodies this union combines yet: \
                 select it again once the model has rebuilt"
            )
        );
        assert_eq!(held(&dialog), [Some("aaaaaaaa"), Some("bbbbbbbb")]);
        // Not on its own body: the body picked, read against nothing.
        let elsewhere = toggle(true, &edge("cccccccc", 1), "cccccccc");
        assert_eq!(
            dialog.took_body(&elsewhere, false, &|_| panic!("read")),
            Taken::Added
        );
        assert_eq!(held(&dialog), [Some("cccccccc"), Some("bbbbbbbb")]);
        assert_eq!(said_under(&dialog, "Target Body"), None);
        // A tool that isn't an ID: not a union it loads.
        let bad = json::read(
            br#"{"id": "n0000001", "type": "fuse", "v": 1, "name": "U", "base": "aaaaaaaa", "tool": 7}"#,
        )
        .unwrap();
        assert!(Dialog::load(Kind::Union, fid("n0000001"), &bad, 2).is_none());
    }

    #[test]
    fn a_browser_row_fills_the_active_body_box_unless_its_no_body_or_built_through_the_boolean() {
        let mut dialog = Dialog::new(Kind::Intersect, None, 1);
        assert_eq!(
            dialog.took_row(&fid("s0000001"), false, false),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Target Body"),
            refused("s0000001 isn't a body: an intersection takes bodies")
        );
        assert_eq!(
            dialog.took_row(&fid("n0000001"), true, true),
            Taken::Refused
        );
        assert_eq!(
            said_under(&dialog, "Target Body"),
            refused(
                "n0000001 is built through this intersection: select one of the bodies it \
                 combines, or another body"
            )
        );
        assert_eq!(held(&dialog), [None, None]);
        assert_eq!(dialog.active, Some(0));
        assert_eq!(dialog.took_row(&fid("k7f2qx3m"), true, false), Taken::Added);
        assert_eq!(said_under(&dialog, "Target Body"), None);
        assert_eq!(held(&dialog), [Some("k7f2qx3m"), None]);
        // A sweep's dialog has no box of one body: a row is its sketch box's, as before.
        let mut pad = Dialog::new(Kind::Pad, None, 2);
        assert_eq!(pad.took_row(&fid("k7f2qx3m"), true, false), Taken::Ignored);
        assert_eq!(pad.inputs[0].value, Value::Selection(None));
    }

    #[test]
    fn the_bodies_selected_as_it_opens_fill_the_target_then_the_tool_and_the_rest_is_noted() {
        // None selected: both empty, the target's box active.
        let mut dialog = Dialog::new(Kind::Union, None, 1);
        dialog.take_bodies(&[], None);
        assert_eq!(held(&dialog), [None, None]);
        assert_eq!(dialog.active, Some(0));
        // One: the target; the tool's box active; what else was selected noted under the
        // target's, in the panel's subdued text.
        let mut dialog = Dialog::new(Kind::Union, None, 2);
        dialog.take_bodies(
            &[fid("k7f2qx3m")],
            Some("1 face left out: a union takes two bodies".to_owned()),
        );
        assert_eq!(held(&dialog), [Some("k7f2qx3m"), None]);
        assert_eq!(dialog.active, Some(1));
        assert_eq!(
            said_under(&dialog, "Target Body"),
            Some(Says::Noted(
                "1 face left out: a union takes two bodies".to_owned()
            ))
        );
        assert_eq!(said_under(&dialog, "Tool Body"), None);
        // A pick lets the note go.
        dialog.took_row(&fid("a91cw0de"), true, false);
        assert_eq!(said_under(&dialog, "Target Body"), None);
        // Two: the target, then the tool; the target's box active.
        let mut dialog = Dialog::new(Kind::Subtract, None, 3);
        dialog.take_bodies(&[fid("a91cw0de"), fid("k7f2qx3m")], None);
        assert_eq!(held(&dialog), [Some("a91cw0de"), Some("k7f2qx3m")]);
        assert_eq!(dialog.active, Some(0));
        assert_eq!(dialog.bodies(), [fid("a91cw0de"), fid("k7f2qx3m")]);
        assert!(dialog.valid());
    }
}
