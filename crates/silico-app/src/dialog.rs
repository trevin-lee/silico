//! Feature dialogs (M5.8; docs/design/m5.0-app.md, sections 8 and 9, *Feature dialogs*):
//! Fusion's docked dialog, a vertical stack of labelled inputs with OK and Cancel at its
//! bottom, of the primitives and of the features that sweep a sketch's profile. A dialog
//! holds what its inputs say; the tab turns each change into an edit of the document
//! inside its transaction ([`crate::tab::Tab`]), so the preview is the model rebuilt.
//!
//! Every input sets one part of one field of the feature (`set_field`'s field): a number,
//! a choice, a toggle, or a selection box, which the browser fills now and a pick in the
//! canvas will fill too (M5.7), as one more source of the same input.

use std::collections::BTreeMap;

use eframe::egui::{self, RichText, Ui};
use silico_core::document::FeatureId;
use silico_core::document::json::{self, Node, Value as Json};

/// The features a dialog makes or edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl Kind {
    /// Every kind, in the toolbar's order.
    pub const ALL: [Kind; 9] = [
        Kind::Box,
        Kind::Cylinder,
        Kind::Sphere,
        Kind::Cone,
        Kind::Torus,
        Kind::Pad,
        Kind::Pocket,
        Kind::Revolve,
        Kind::Groove,
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
        }
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
    /// A selection box: the feature picked, by its ID, if one is. The browser fills it now;
    /// a pick in the canvas fills it too once there's one (M5.7).
    Selection(Option<FeatureId>),
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
    /// The body a new one's add takes as its `base`.
    base: Option<FeatureId>,
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
    /// that does and one is given, and taking `base` as its body if it's one that can.
    pub fn new(
        kind: Kind,
        sketch: Option<FeatureId>,
        base: Option<FeatureId>,
        serial: u64,
    ) -> Self {
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
        let active = inputs
            .iter()
            .position(|input| matches!(input.value, Value::Selection(_)));
        Dialog {
            kind,
            feature: None,
            made: true,
            inputs,
            base: base.filter(|_| kind.sweeps()),
            sent: BTreeMap::new(),
            adding: false,
            active,
            serial,
        }
    }

    /// The dialog of the existing feature `id`, of `kind`, its inputs loaded from `node`,
    /// the feature as the document writes it (`text` of it): none if it isn't one a
    /// dialog of `kind` reads. Nothing is sent until an input changes.
    pub fn load(kind: Kind, id: FeatureId, node: &Node, serial: u64) -> Option<Self> {
        let mut dialog = Dialog::new(kind, None, None, serial);
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
            };
            match input.part {
                Part::Position(at) => match (text, position.get_mut(at)) {
                    (Some(text), Some(slot)) => *slot = text,
                    _ => position_ok = false,
                },
                Part::Number(_) | Part::Sketch | Part::Axis => {
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
            }
        }
        if self.kind.sweeps() {
            let members: Vec<String> = extent
                .iter()
                .map(|(field, text)| format!("{}: {text}", quoted(field)))
                .collect();
            fields.insert(
                "extent",
                extent_ok.then(|| format!("{{{}}}", members.join(", "))),
            );
        } else {
            fields.insert(
                "position",
                position_ok.then(|| format!("[{}]", position.join(", "))),
            );
        }
        fields
    }

    /// Whether every field can be sent: OK is off until it can.
    pub fn valid(&self) -> bool {
        self.fields().values().all(Option::is_some)
    }

    /// The new feature's add, as the document writes a feature (no `id`, for the core to
    /// give it one), if it's to be sent now: none once its feature is there, while one is
    /// under way (what changes meanwhile is sent as edits once it has answered), while the
    /// inputs aren't valid, or while they're what the last add, refused, sent.
    pub fn add(&mut self) -> Option<Node> {
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
        if let Some(base) = &self.base {
            members.push(format!("\"base\": {}", quoted(base.as_str())));
        }
        members.extend(
            fields
                .iter()
                .map(|(field, text)| format!("{}: {text}", quoted(field))),
        );
        let node = json::read(format!("{{{}}}", members.join(", ")).as_bytes()).ok()?;
        self.sent = fields;
        self.adding = true;
        Some(node)
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

    /// The dialog drawn into `ui`: its title, its inputs, `said` (its feature's error, a
    /// refusal), and OK and Cancel; `named` gives a feature's name by its ID (a selection
    /// box shows it), `refused` whether an edit of a field was refused (its input red, OK
    /// off), and `enabled` whether anything can be sent now (off while the tab closes).
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        said: &[String],
        named: &dyn Fn(&FeatureId) -> Option<String>,
        refused: &dyn Fn(&str) -> bool,
        enabled: bool,
    ) -> Action {
        ui.label(RichText::new(self.kind.title().to_uppercase()).strong());
        ui.separator();
        let error = ui.visuals().error_fg_color;
        let shown: Vec<bool> = self.inputs.iter().map(|input| self.shown(input)).collect();
        let readable: Vec<bool> = self
            .inputs
            .iter()
            .map(|input| self.readable(input))
            .collect();
        let mut any_refused = false;
        let active = self.active;
        egui::Grid::new(("dialog", self.serial))
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for (at, input) in self.inputs.iter_mut().enumerate() {
                    if !shown.get(at).copied().unwrap_or(true) {
                        continue;
                    }
                    let red =
                        !readable.get(at).copied().unwrap_or(true) || refused(input.part.field());
                    any_refused |= refused(input.part.field());
                    let label = ui.label(input.label);
                    let unit = match input.value {
                        Value::Length(_) => "mm",
                        Value::Angle(_) => "°",
                        Value::Toggle(_) | Value::Choice { .. } | Value::Selection(_) => "",
                    };
                    match &mut input.value {
                        Value::Length(text) | Value::Angle(text) => {
                            let mut edit = egui::TextEdit::singleline(text)
                                .id_salt(("dialog-input", input.label))
                                .desired_width(120.0);
                            if red {
                                edit = edit.text_color(error);
                            }
                            ui.add_enabled(enabled, edit).labelled_by(label.id);
                            ui.label(unit);
                        }
                        Value::Toggle(on) => {
                            ui.add_enabled(enabled, egui::Checkbox::without_text(on))
                                .labelled_by(label.id);
                            ui.label("");
                        }
                        Value::Choice { options, chosen } => {
                            ui.horizontal(|ui| {
                                for (place, (_, shown)) in options.iter().enumerate() {
                                    let button = egui::Button::selectable(*chosen == place, shown);
                                    if ui.add_enabled(enabled, button).clicked() {
                                        *chosen = place;
                                    }
                                }
                            });
                            ui.label("");
                        }
                        Value::Selection(picked) => {
                            let (text, colour) = match picked.as_ref() {
                                Some(id) => (
                                    named(id).unwrap_or_else(|| id.as_str().to_owned()),
                                    ui.visuals().text_color(),
                                ),
                                None => ("Select in the browser".to_owned(), error),
                            };
                            // The active selection box, highlighted (section 9).
                            let stroke = if active == Some(at) {
                                ui.visuals().selection.stroke
                            } else {
                                ui.visuals().widgets.noninteractive.bg_stroke
                            };
                            egui::Frame::new()
                                .stroke(stroke)
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| ui.colored_label(colour, text));
                            ui.label("");
                        }
                    }
                    ui.end_row();
                }
            });
        for said in said {
            ui.colored_label(error, said);
        }
        ui.separator();
        let mut action = Action::None;
        ui.horizontal(|ui| {
            let ok = enabled && self.valid() && !any_refused;
            if ui.add_enabled(ok, egui::Button::new("OK")).clicked() {
                action = Action::Ok;
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Cancel"))
                .clicked()
            {
                action = Action::Cancel;
            }
        });
        action
    }
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
        let mut dialog = Dialog::new(Kind::Box, None, None, 1);
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
        let again = dialog.add().unwrap();
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
        let mut pocket = Dialog::new(Kind::Pocket, None, None, 1);
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
        let angle = Dialog::new(Kind::Revolve, None, None, 2);
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
    fn a_pick_fills_only_the_active_selection_box() {
        let mut pad = Dialog::new(Kind::Pad, None, None, 1);
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
        let mut cube = Dialog::new(Kind::Box, None, None, 2);
        assert_eq!(cube.active, None);
        let before = cube.clone();
        cube.pick(&FeatureId::new("s0000001").unwrap());
        assert_eq!(cube, before);
    }

    #[test]
    fn a_sweep_with_no_sketch_isnt_valid_and_a_primitive_takes_no_base() {
        let base = FeatureId::new("b0000001").unwrap();
        let mut pad = Dialog::new(Kind::Pad, None, Some(base.clone()), 1);
        assert!(!pad.valid());
        assert!(pad.add().is_none());
        pad.pick(&FeatureId::new("s0000001").unwrap());
        assert!(pad.valid());
        let mut cube = Dialog::new(Kind::Box, None, Some(base), 2);
        let added = cube.add().unwrap();
        assert!(!format!("{:?}", json::Plain::of(&added)).contains("b0000001"));
    }
}
