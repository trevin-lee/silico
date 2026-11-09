//! Smart Dimension's canvas (docs/design/m5.0-app.md, section 9, *Smart Dimension* and *A
//! dimension that over-defines*; SD1 to SD9): the Modify box, the over-define prompt (SD7),
//! and the edit of a placed dimension. What's placed, how a value reads, and when a
//! dimension over-defines are the core's (`sketch_tools::SmartDimension`, #345, and its
//! [`offer`]); here is only what's drawn of them and what's sent.
//!
//! - **The Modify box** takes what's typed raw and whole, never trimmed, and both boxes read
//!   it the same way, by the core's `read_typed` (SD9): Smart Dimension's inside the core, as
//!   `DimensionEvent::Typed`; a placed dimension's here ([`edit_value`]), into its
//!   `set_field` value. A refusal is said in the box, with nothing sent.
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

use eframe::egui::{self, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Stroke, Ui, Vec2};
use silico_core::command::sketch::SketchRead;
use silico_core::command::{Revision, SessionId};
use silico_core::document::{ElementId, FeatureId, Form};
use silico_core::sketch_tools::{
    Offer, OfferChoice, OfferRequest, Placement, PlacementKind, offer, read_typed,
};

use super::style::*;
use crate::icons::{self, Icon};

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
    /// Whether the read it was made on had its solve refused (`PlacementKind::Edit`'s).
    pub before_refused: bool,
    pub made: Option<Revision>,
}

impl Edited {
    /// Of `constraint`, of the sketch `read` read at the revision `model`: the read's state,
    /// the constraints it flags, and whether its solve was refused, which the core's
    /// [`offer`] judges an edit by (`PlacementKind::Edit`).
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
            before_refused: read.states.solve_problem.is_some(),
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
            kind: PlacementKind::Edit {
                before_refused: self.before_refused,
            },
        };
        Some(offer(read, made, &placement))
    }
}

/// What a placed dimension's Modify box sends of `text`, typed for the dimension of form
/// `form`: the `set_field` value of its `value`, as the core's `read_typed` reads it (as
/// Smart Dimension's own box reads its text), or what it says of the text, refused.
pub fn edit_value(form: &Form, text: &str) -> Result<silico_core::document::json::Node, String> {
    read_typed(form, text)
        .map(|typed| typed.node())
        .map_err(|message| message.text())
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

/// Where a box `size` big, wanted with its top left at `wanted`, stands in the viewport
/// `within`: whole inside it, and, if it would cover the Sketch Palette `palette`, moved
/// left of it, or else up above it, [`SKETCH_PALETTE_CLEARANCE`] from it. A box that fits
/// neither way (a viewport too small for both) stays whole inside the viewport, at its
/// left, as near its place as it can be.
pub fn clear_of_palette(wanted: Pos2, size: Vec2, within: Rect, palette: Rect) -> Pos2 {
    // Inside `within` along one axis: from `low` to `high`, a box `extent` long; one longer
    // than the room, at `low`.
    let inside = |at: f32, extent: f32, low: f32, high: f32| at.min(high - extent).max(low);
    let into = |p: Pos2| {
        Pos2::new(
            inside(p.x, size.x, within.left(), within.right()),
            inside(p.y, size.y, within.top(), within.bottom()),
        )
    };
    let at = into(wanted);
    let kept = palette.expand(SKETCH_PALETTE_CLEARANCE);
    if !Rect::from_min_size(at, size).intersects(kept) {
        return at;
    }
    let left = Pos2::new(kept.left() - size.x, at.y);
    if left.x >= within.left() {
        return left;
    }
    let up = Pos2::new(at.x, kept.top() - size.y);
    if up.y >= within.top() {
        return up;
    }
    Pos2::new(within.left(), at.y)
}

/// What names a sketch mode's Modify boxes: its tab's session `session` and its opening
/// `serial`, so no two sketch modes' boxes are one box to egui, though each numbers its
/// boxes from the start ([`modify_box`]'s `owner`).
pub fn modify_owner(session: &SessionId, serial: u64) -> egui::Id {
    egui::Id::new(("sketch_mode", session.to_string(), serial))
}

/// The area of the Modify box numbered `opened` of the sketch mode `owner` names: where
/// egui keeps its place and its size.
pub fn modify_area(owner: egui::Id, opened: u64) -> egui::Id {
    owner.with(("sketch_modify", opened))
}

/// The Modify box (the prototype's `.modify-box`, "Modify dimension"): the value's field, in
/// [`field_frame`]'s Modify look, its unit, ✓ and ✕, and under it a hint, or what was said
/// of the text, in red; at its dimension's text, the box and its hint whole inside the
/// viewport `viewport` and clear of the Sketch Palette `palette` ([`clear_of_palette`], by
/// its size as last drawn). A new box takes the keyboard. What it was asked, if anything.
///
/// `owner` names the sketch mode the box is of (its tab's session and its opening, as the
/// app gives it): a box is known by `owner` and its number, so each box of every sketch
/// mode takes the keyboard once as it opens, a later sketch's as the first's, though each
/// sketch mode numbers its boxes from the start again.
pub fn modify_box(
    ctx: &egui::Context,
    owner: egui::Id,
    view: &ModifyView,
    viewport: Rect,
    palette: Rect,
) -> Option<ModifyAction> {
    let mut asked = None;
    let id = modify_area(owner, view.opened);
    // Its size as last drawn; none before its first (egui's sizing pass, drawn unseen).
    let size = egui::AreaState::load(ctx, id).and_then(|state| state.size);
    let wanted = view.at + MODIFY_FROM_TEXT;
    let at = size.map_or(wanted, |size| {
        clear_of_palette(wanted, size, viewport, palette)
    });
    egui::Area::new(id)
        .fixed_pos(at)
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
                                            .char_limit(MODIFY_CHAR_LIMIT)
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
                        let taken = owner.with(("sketch_modify_taken", view.opened));
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
                        if icon_button(ui, Icon::Check, "Accept", !view.waiting)
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
                        if icon_button(ui, Icon::Close, "Cancel", true)
                            .on_hover_text(skip)
                            .clicked()
                        {
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
    // Placed by a size it no longer has (its hint grew, say): placed again next frame.
    if size.is_some() && egui::AreaState::load(ctx, id).and_then(|state| state.size) != size {
        ctx.request_repaint();
    }
    asked
}

/// One of the Modify box's buttons (the prototype's `.ab-btn`, [`MODIFY_BUTTON_SIZE`]
/// square): `icon`, from the app's set, named `name` for the accessibility tree, as the
/// prototype's `aria-label`.
fn icon_button(ui: &mut Ui, icon: Icon, name: &str, enabled: bool) -> egui::Response {
    let button = egui::Button::image(icons::icon(icon))
        .fill(Color32::TRANSPARENT)
        .min_size(Vec2::splat(MODIFY_BUTTON_SIZE));
    let response = ui.add_enabled(enabled, button);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    response
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
                .inner_margin(SKETCH_PROMPT_MARGIN),
        )
        .show(ctx, |ui| {
            *ui.visuals_mut() = super::bar::panel_visuals();
            // The prototype's 440 wide, its border and its sides' padding inside it.
            let margin = SKETCH_PROMPT_MARGIN.left + SKETCH_PROMPT_MARGIN.right;
            ui.set_width(SKETCH_PROMPT_WIDTH - 2.0 - f32::from(margin));
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Dialog);
                node.set_label("Dimension over-defines the sketch");
            });
            // Its head (`.m-head`): the warning's icon, then the title, 8 apart.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add(icons::icon(Icon::Warn).tint(TEXT_STRONG));
                ui.label(
                    RichText::new(format!("This dimension over-defines {}", view.sketch))
                        .size(14.0)
                        .color(TEXT_STRONG),
                );
            });
            ui.add_space(8.0);
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
            ui.add_space(12.0);
            ui.spacing_mut().item_spacing.y = SKETCH_PROMPT_CHOICE_GAP;
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

/// One of the prompt's choices (the prototype's `.choice`): the prompt's whole width, its
/// title and its words in a column that wraps them, and its key's badge ([`key_badge`]) at
/// the top right, [`SKETCH_PROMPT_CHOICE_PADDING`]'s gap from them; the whole row a button.
/// Whether it was clicked.
fn choice_row(ui: &mut Ui, title: &str, words: &str, key: &str) -> bool {
    let padding = SKETCH_PROMPT_CHOICE_PADDING;
    let frame = egui::Frame::new()
        .stroke(Stroke::new(1.0, SKETCH_PROMPT_CHOICE_STROKE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(padding);
    // Inside its border and padding, the row's width shared by the words and the badge.
    let inner = ui.available_width() - 2.0 - f32::from(padding.left) - f32::from(padding.right);
    let badge = key_size(ui, key);
    let shown = frame.show(ui, |ui| {
        ui.set_width(inner);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = SKETCH_PROMPT_CHOICE_GAP_X;
            let column = (inner - badge.x - SKETCH_PROMPT_CHOICE_GAP_X).max(0.0);
            ui.vertical(|ui| {
                ui.set_width(column);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(RichText::new(title).color(TEXT_STRONG)).wrap());
                ui.add(
                    egui::Label::new(
                        RichText::new(words)
                            .size(CANVAS_LABEL_SIZE)
                            .color(TEXT_SUBDUED),
                    )
                    .wrap(),
                );
            });
            key_badge(ui, key, badge);
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

/// A key's badge's size (the prototype's `.kbd`): [`SKETCH_KEY_HEIGHT`] high, and as wide
/// as its text and 4 each side, its border too, but never narrower than it is high.
fn key_size(ui: &Ui, key: &str) -> Vec2 {
    let text = ui.painter().layout_no_wrap(
        key.to_owned(),
        egui::FontId::proportional(SKETCH_MODIFY_HINT_SIZE),
        TEXT_DEFAULT,
    );
    let width = (text.size().x + 2.0 * 4.0 + 2.0).max(SKETCH_KEY_HEIGHT);
    Vec2::new(width, SKETCH_KEY_HEIGHT)
}

/// A key's badge (the prototype's `.kbd`), `size` big: Gray.250, its border Gray.350 and 2
/// at its foot, its corners 3, and the key centred, in the hint's size; a label in the
/// accessibility tree.
fn key_badge(ui: &mut Ui, key: &str, size: Vec2) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, key));
    let painter = ui.painter();
    let corners = CornerRadius::same(3);
    // The border's colour under the whole badge, then its fill inset 1 at the top and
    // sides and 2 at its foot.
    painter.rect_filled(rect, corners, SKETCH_KEY_STROKE);
    let inside =
        egui::Rect::from_min_max(rect.min + Vec2::splat(1.0), rect.max - Vec2::new(1.0, 2.0));
    painter.rect_filled(inside, CornerRadius::same(2), SKETCH_KEY_BG);
    painter.text(
        inside.center(),
        egui::Align2::CENTER_CENTER,
        key,
        egui::FontId::proportional(SKETCH_MODIFY_HINT_SIZE),
        SKETCH_KEY_TEXT,
    );
}
