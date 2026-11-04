//! Smart Dimension's canvas (docs/design/m5.0-app.md, section 9, *Smart Dimension* and *A
//! dimension that over-defines*; SD1 to SD9): the Modify box, the over-define prompt (SD7),
//! and the edit of a placed dimension. What's placed, how a value reads, and when a
//! dimension over-defines are the core's (`sketch_tools::SmartDimension`, #345, and its
//! [`offer`]); here is only what's drawn of them and what's sent.
//!
//! - **The Modify box** takes what's typed raw and whole: Smart Dimension's as
//!   `DimensionEvent::Typed`, a placed dimension's as the text of its `set_field`, read by the
//!   core's own reader, never trimmed or parsed here (SD9). A refusal is said in the box.
//! - **The prompt** offers SolidWorks' three choices for a dimension that over-defines,
//!   placed or edited: Driven (`set_field` of its `driving` to false), Leave (nothing), and
//!   Cancel (`undo`), each made against the revision its add or edit made, through the tab's
//!   queue. A `conflict` answer means the session moved on first: overtaken, said, and never
//!   sent again.
//! - **Editing a placed dimension:** a double-click on it opens the box with its value; the
//!   text typed is sent as a `set_field` of its `value`, and an edit that over-defines gets
//!   the same three choices, judged by the core's [`offer`] on the read of the revision it
//!   made, as a placement is.

use std::collections::BTreeSet;

use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, RichText, Stroke, Ui, Vec2};
use silico_core::command::Revision;
use silico_core::command::sketch::SketchRead;
use silico_core::document::{ElementId, FeatureId, Form};
use silico_core::sketch_tools::{Offer, OfferChoice, OfferRequest, Placement, offer};

use super::style::*;

/// What sketch mode asks of the core besides its tools' requests, in the order it asks: a
/// placed dimension's value (`set_field` of its `value`), its text as the core's reader read
/// it, whole; and an offer's choice.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Value {
        sketch: FeatureId,
        constraint: ElementId,
        value: silico_core::document::json::Node,
    },
    Offer(OfferRequest),
}

/// A placed dimension's Modify box, open: the constraint, its text as typed, its unit,
/// where on the plane it opened, its number (what its focus is keyed on), whether its text
/// was sent and not yet answered, and what the core said of the last text sent.
#[derive(Debug, Clone, PartialEq)]
pub struct EditBox {
    pub constraint: ElementId,
    pub text: String,
    pub unit: &'static str,
    pub at: [f64; 2],
    pub opened: u64,
    pub sent: bool,
    pub said: Option<String>,
}

/// An edit of a placed dimension sent, to be judged once the read of the revision it made
/// comes: its placement, as the core's [`offer`] takes one, its revision once answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Edited {
    pub sketch: FeatureId,
    pub constraint: ElementId,
    pub read: Revision,
    pub before: silico_core::sketch_states::SketchState,
    pub flagged: BTreeSet<ElementId>,
    pub dofs: i32,
    pub made: Option<Revision>,
}

impl Edited {
    /// Of `constraint`, of the sketch `read` read at the revision `model`. Its degrees of
    /// freedom are given as `i32::MAX`, past any a read has: [`offer`]'s dependent add (one
    /// that leaves the freedoms where they were) is an add's rule, since a driving dimension
    /// added adds an equation; an edit of a value adds none, and leaves them where they were
    /// always. So an edit offers only where it flags a constraint the read before didn't, or
    /// takes the sketch to over defined or no solution, as the rest of [`offer`] has it.
    pub fn of(read: &SketchRead, model: Revision, constraint: ElementId) -> Self {
        Edited {
            sketch: read.sketch.clone(),
            constraint,
            read: model,
            before: read.states.state,
            flagged: read
                .states
                .constraints
                .iter()
                .filter(|(_, state)| state.flagged())
                .map(|(id, _)| id.clone())
                .collect(),
            dofs: i32::MAX,
            made: None,
        }
    }

    /// A read of the model at `model`: none yet (its edit unanswered, or the read older), or
    /// its judgement: the offer the core's [`offer`] gives on the read of the revision the
    /// edit made, if it gives one; none on any later read, the moment past.
    pub fn judged(&self, read: &SketchRead, model: Revision) -> Option<Option<Offer>> {
        let made = self.made?;
        if model < made {
            return None;
        }
        if model > made {
            return Some(None);
        }
        let placement = Placement {
            sketch: self.sketch.clone(),
            constraint: self.constraint.clone(),
            revision: made,
            read: self.read,
            before: self.before,
            flagged: self.flagged.clone(),
            dofs: self.dofs,
        };
        Some(offer(read, made, &placement))
    }
}

/// A dimension's value as the document writes it, and its unit: the Modify box's text when
/// it opens on a placed one. None for a form that isn't a dimension.
pub fn value_of(form: &Form) -> Option<(String, &'static str)> {
    Some(match form {
        Form::Angle { value, .. } => (value.to_string(), "°"),
        Form::Length { value, .. }
        | Form::Distance { value, .. }
        | Form::DistanceX { value, .. }
        | Form::DistanceY { value, .. }
        | Form::PointLineDistance { value, .. }
        | Form::Radius { value, .. }
        | Form::Diameter { value, .. } => (value.to_string(), "mm"),
        _ => return None,
    })
}

/// What the Modify box was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModifyAction {
    /// Its text, whole, as it now reads.
    Typed(String),
    /// Enter, or its ✓.
    Accept,
    /// Its ✕: Smart Dimension's box skipped (the measured value placed, SD2), a placed
    /// dimension's closed with no change.
    Skip,
}

/// What the Modify box shows: its text, its unit, where it stands on the screen, its number
/// (a new box takes the keyboard once), what was said of its text, whether it's waiting on
/// the core, and whether it's a new dimension's (whose ✕ skips) or a placed one's.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifyView {
    pub text: String,
    pub unit: &'static str,
    pub at: Pos2,
    pub opened: u64,
    pub said: Option<String>,
    pub waiting: bool,
    pub placing: bool,
}

/// How far the Modify box stands from its dimension's text, as the prototype puts it: its
/// field's middle at the text, a little left and up.
const MODIFY_FROM_TEXT: Vec2 = Vec2::new(-74.0, -15.0);

/// The Modify box (the prototype's `.modify-box`, "Modify dimension"): the value's field, in
/// [`field_frame`]'s Modify look, its unit, ✓ and ✕, and under it a hint, or what was said
/// of the text, in red. A new box takes the keyboard. What it was asked, if anything.
pub fn modify_box(ctx: &egui::Context, view: &ModifyView) -> Option<ModifyAction> {
    let mut asked = None;
    egui::Area::new(egui::Id::new(("sketch_modify", view.opened)))
        .fixed_pos(view.at + MODIFY_FROM_TEXT)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            *ui.visuals_mut() = super::bar::panel_visuals();
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Dialog);
                node.set_label("Modify dimension");
            });
            egui::Frame::new()
                .fill(SKETCH_MODIFY_BG)
                .stroke(Stroke::new(1.0, SKETCH_MODIFY_STROKE))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(3))
                .shadow(egui::Shadow {
                    offset: [0, 6],
                    blur: 20,
                    spread: 0,
                    color: Color32::from_black_alpha(153),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let id = egui::Id::new(("sketch_modify_field", view.opened));
                        let focused = ui.memory(|m| m.has_focus(id));
                        let mut text = view.text.clone();
                        let field =
                            field_frame(FieldKind::Modify, false, focused, view.said.is_some())
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut text)
                                            .id(id)
                                            .frame(egui::Frame::NONE)
                                            .desired_width(SKETCH_MODIFY_FIELD_WIDTH)
                                            .text_color(if view.said.is_some() {
                                                ERROR_TEXT
                                            } else {
                                                TEXT_STRONG
                                            }),
                                    )
                                    .on_hover_text(format!("Value in {}", unit_words(view.unit)))
                                })
                                .inner;
                        ui.ctx().accesskit_node_builder(field.id, |node| {
                            node.set_label(format!("Value in {}", unit_words(view.unit)));
                        });
                        // A new box takes the keyboard, once, its cursor after its value.
                        // (Not selected, as SolidWorks' and the prototype's is: egui 0.36's
                        // field collapses a selection made outside its own events.)
                        let taken = egui::Id::new(("sketch_modify_taken", view.opened));
                        if !ui.data(|d| d.get_temp::<bool>(taken).unwrap_or(false)) {
                            field.request_focus();
                            ui.data_mut(|d| d.insert_temp(taken, true));
                        }
                        if text != view.text {
                            asked = Some(ModifyAction::Typed(text));
                        }
                        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            asked = Some(ModifyAction::Accept);
                        }
                        ui.label(RichText::new(view.unit).color(TEXT_SUBDUED));
                        if ui
                            .add_enabled(!view.waiting, egui::Button::new("✓"))
                            .on_hover_text("Accept (Enter)")
                            .clicked()
                        {
                            asked = Some(ModifyAction::Accept);
                        }
                        let skip = if view.placing {
                            "Skip: the measured value is placed (Esc)"
                        } else {
                            "Cancel: the value stays (Esc)"
                        };
                        if ui.button("✕").on_hover_text(skip).clicked() {
                            asked = Some(ModifyAction::Skip);
                        }
                    });
                });
            let hint = match &view.said {
                Some(said) => RichText::new(said).color(ERROR_TEXT),
                None if view.waiting => RichText::new("Applying…").color(TEXT_SUBDUED),
                None => RichText::new(format!("A plain number in {} · Enter accepts", view.unit))
                    .color(TEXT_SUBDUED),
            };
            ui.label(hint.size(SKETCH_MODIFY_HINT_SIZE));
        });
    asked
}

/// A unit's words.
fn unit_words(unit: &str) -> &'static str {
    if unit == "°" {
        "degrees"
    } else {
        "millimetres"
    }
}

/// What the over-define prompt shows: the sketch's name, the dimension's value as the
/// document writes it, and the constraints the sketch's read flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferView {
    pub sketch: String,
    pub value: Option<String>,
    pub flagged: Vec<String>,
}

/// The over-define prompt (SD7; the prototype's modal "This dimension over-defines …"): a
/// word on what the dimension repeats, the constraints flagged, and SolidWorks' three
/// choices, each with its key (1, 2, and Esc, which the tab's Esc takes). The choice made,
/// if one was.
pub fn offer_prompt(ctx: &egui::Context, view: &OfferView) -> Option<OfferChoice> {
    let mut chose = None;
    egui::Modal::new(egui::Id::new("sketch_offer"))
        .backdrop_color(Color32::from_black_alpha(153))
        .frame(
            egui::Frame::new()
                .fill(FLOATING_BG)
                .stroke(Stroke::new(1.0, PANEL_BORDER))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(16)),
        )
        .show(ctx, |ui| {
            *ui.visuals_mut() = super::bar::panel_visuals();
            ui.set_width(SKETCH_PROMPT_WIDTH);
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Dialog);
                node.set_label("Dimension over-defines the sketch");
            });
            ui.label(
                RichText::new(format!("This dimension over-defines {}", view.sketch))
                    .size(14.0)
                    .color(TEXT_STRONG),
            );
            ui.add_space(6.0);
            let value = view
                .value
                .as_deref()
                .map_or_else(|| "It".to_owned(), |value| format!("Its value, {value},"));
            ui.label(
                RichText::new(format!(
                    "The sketch can't take it as well as what it has. {value} conflicts with or repeats what the others give."
                ))
                .color(TEXT_DEFAULT),
            );
            if !view.flagged.is_empty() {
                ui.label(
                    RichText::new(format!("Flagged: {}", view.flagged.join(", ")))
                        .size(SKETCH_MODIFY_HINT_SIZE)
                        .color(ERROR_TEXT),
                );
            }
            ui.add_space(10.0);
            for (choice, title, words, key) in [
                (
                    OfferChoice::Driven,
                    "Make it driven",
                    "Shows the measured value in parentheses, and constrains nothing.",
                    "1",
                ),
                (
                    OfferChoice::Leave,
                    "Leave it driving",
                    "The sketch is over defined, drawn red, and what's built on it won't build until a dimension goes.",
                    "2",
                ),
                (OfferChoice::Cancel, "Cancel", "Removes the dimension.", "Esc"),
            ] {
                if choice_row(ui, title, words, key) {
                    chose = Some(choice);
                }
            }
            let key = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
            if key(egui::Key::Num1) {
                chose = Some(OfferChoice::Driven);
            } else if key(egui::Key::Num2) {
                chose = Some(OfferChoice::Leave);
            }
        });
    chose
}

/// One of the prompt's choices (the prototype's `.choice`): its title, its words, and its
/// key at the right, the whole row a button. Whether it was clicked.
fn choice_row(ui: &mut Ui, title: &str, words: &str, key: &str) -> bool {
    let frame = egui::Frame::new()
        .stroke(Stroke::new(1.0, SKETCH_PROMPT_CHOICE_STROKE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(TEXT_STRONG));
                ui.label(
                    RichText::new(words)
                        .size(CANVAS_LABEL_SIZE)
                        .color(TEXT_SUBDUED),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::Frame::new()
                    .fill(SKETCH_KEY_BG)
                    .stroke(Stroke::new(1.0, SKETCH_KEY_STROKE))
                    .corner_radius(CornerRadius::same(3))
                    .inner_margin(Margin::symmetric(4, 0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(key)
                                .size(SKETCH_MODIFY_HINT_SIZE)
                                .color(TEXT_DEFAULT),
                        );
                    });
            });
        });
    });
    let response = ui.interact(
        shown.response.rect,
        ui.id().with(title),
        egui::Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    response.clicked()
}
