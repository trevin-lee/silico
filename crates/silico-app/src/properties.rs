//! The property panel's parts (docs/design/m5-ui/DESIGN.md, *Layout*: one right panel, four
//! jobs; its `.panel-title`, `.prop-head`, `.section`, `.prop`, `.hint-block`, `.err-card`,
//! `.selbox`, `.seg`, and `.dlg-footer` rules, and `renderRight`, `docProps`,
//! `featureProps`, and `renderDialog`): what [`crate::panes`] draws the selected feature's
//! properties, the document's, and the docked feature dialog (`crate::dialog`) with.
//!
//! Every text is a label, so the accessibility tree has it as it's drawn.

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Label, Layout, Margin, Rect, Response,
    Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use silico_core::document::FeatureId;

use crate::icons::{Icon, IconSize};
use crate::modal::{self, Kind};
use crate::tab::Said;
use crate::theme;

/// The panel's padding at its sides (`--pad`).
fn pad() -> f32 {
    f32::from(theme::VIEW_PADDING)
}

/// Text in `size` points, monospace if `mono`, in `colour`.
pub(crate) fn format(size: f32, colour: Color32, mono: bool) -> TextFormat {
    TextFormat {
        font_id: FontId::new(
            size,
            if mono {
                FontFamily::Monospace
            } else {
                FontFamily::Proportional
            },
        ),
        color: colour,
        ..Default::default()
    }
}

/// `text` in the body's type (`--fs`), in `colour`.
pub(crate) fn body(text: &str, colour: Color32) -> LayoutJob {
    LayoutJob::single_section(text.to_owned(), format(theme::BODY_SIZE, colour, false))
}

/// `add`, in `rect`, from its left, centred on its height, in a child of `ui` of its own,
/// named by `salt` among `ui`'s: `ui`'s cursor stays where it is (the caller has allocated
/// `rect`'s place already).
fn within<R>(
    ui: &mut Ui,
    rect: Rect,
    salt: impl egui::AsIdSalt,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(salt)
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    centring(&mut child, rect);
    add(&mut child)
}

/// `ui`, a row from left to right in `rect`, centring what it adds on `rect`'s height: egui
/// centres a row's widgets on its widgets' height (`interact_size.y`), not its own.
fn centring(ui: &mut Ui, rect: Rect) {
    ui.spacing_mut().interact_size.y = rect.height();
}

/// A field's place (`.field`: [`theme::FIELD_HEIGHT`] high) in `ui`'s row, from where the
/// row has got to, `width` wide, centred on the row's height: for `Ui::put`.
pub(crate) fn field_rect(ui: &Ui, width: f32) -> Rect {
    let row = ui.max_rect();
    Rect::from_min_size(
        pos2(
            ui.cursor().left(),
            row.center().y - theme::FIELD_HEIGHT / 2.0,
        ),
        vec2(width, theme::FIELD_HEIGHT),
    )
}

/// A label of `job`, cut short with an ellipsis where it doesn't fit.
fn truncated(ui: &mut Ui, mut job: LayoutJob) -> Response {
    job.wrap = TextWrapping::truncate_at_width(ui.available_width());
    ui.add(Label::new(job).selectable(false))
}

/// A side panel's title (`.panel-title`, the browser's and the property panel's): `icon`,
/// if it has one (a dialog's), and `text` in capitals, on a [`theme::TITLE_BAR_HEIGHT`]
/// row; and at its right, if `close` names one, a close button (`.panel-title .ab-btn`)
/// labelled so: whether it was clicked.
pub(crate) fn title(ui: &mut Ui, icon: Option<Icon>, text: &str, close: Option<&str>) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::TITLE_BAR_HEIGHT),
        Sense::hover(),
    );
    let inner = Rect::from_min_max(
        pos2(rect.left() + theme::BROWSER_TITLE_PADDING, rect.top()),
        pos2(
            rect.right() - theme::PANEL_TITLE_PADDING_RIGHT,
            rect.bottom(),
        ),
    );
    let mut clicked = false;
    let words = match close {
        Some(label) => {
            let side = theme::PANEL_TITLE_BUTTON_SIZE;
            let button = Rect::from_min_size(
                pos2(inner.right() - side, inner.center().y - side / 2.0),
                Vec2::splat(side),
            );
            clicked = within(ui, button, ("title-close", text), |ui| {
                close_button(ui, label)
            })
            .clicked();
            Rect::from_min_max(
                inner.min,
                pos2(button.left() - theme::PANEL_TITLE_GAP, inner.bottom()),
            )
        }
        None => inner,
    };
    within(ui, words, ("title", text), |ui| {
        ui.spacing_mut().item_spacing = vec2(theme::PANEL_TITLE_GAP, 0.0);
        if let Some(icon) = icon {
            ui.add(icon.image(IconSize::Small).tint(theme::TEXT_STRONG));
        }
        let shown = text.to_uppercase();
        let mut job = LayoutJob::single_section(
            shown.clone(),
            TextFormat {
                extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
                ..format(theme::HEADER_SIZE, theme::TEXT_STRONG, false)
            },
        );
        job.wrap = TextWrapping::truncate_at_width(ui.available_width());
        let galley = ui.painter().layout_job(job);
        let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
        ui.painter().galley(rect.min, galley, theme::TEXT_STRONG);
        // A heading in the accessibility tree, not one more of the pane's labels.
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, &shown));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::Heading);
        });
    });
    clicked
}

/// The title's close button (`.panel-title .ab-btn`): its × in Gray.750, white on Gray.300
/// hovered, labelled `label`, with Cancel's tooltip.
fn close_button(ui: &mut Ui, label: &str) -> Response {
    let side = theme::PANEL_TITLE_BUTTON_SIZE;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    let ink = if response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::SMALL_CORNER_RADIUS,
            theme::APP_BAR_BUTTON_HOVER_BG,
        );
        theme::TEXT_STRONG
    } else {
        theme::TEXT_DEFAULT
    };
    theme::focus_ring(
        ui,
        &response,
        rect,
        theme::SMALL_CORNER_RADIUS,
        StrokeKind::Outside,
    );
    Icon::Close.image(IconSize::Small).tint(ink).paint_at(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(IconSize::Small.points())),
    );
    let cancel = crate::shortcuts::Command::Cancel;
    let said = format!("{} ({})", cancel.label(), cancel.keys_text(ui.ctx()));
    response.on_hover_text(said)
}

/// The head (`.prop-head`): `icon` in its box, `title` over `sub`.
pub(crate) fn head(ui: &mut Ui, icon: Icon, title: &str, sub: LayoutJob) {
    let words = 2.0 * theme::BODY_LINE_HEIGHT;
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            theme::PROPERTY_HEAD_PADDING_TOP + words + theme::PROPERTY_HEAD_PADDING_BOTTOM,
        ),
        Sense::hover(),
    );
    let middle = rect.top() + theme::PROPERTY_HEAD_PADDING_TOP + words / 2.0;
    let side = theme::PROPERTY_HEAD_ICON_SIDE;
    let boxed = Rect::from_min_size(
        pos2(rect.left() + pad(), middle - side / 2.0),
        Vec2::splat(side),
    );
    ui.painter().rect_filled(
        boxed,
        theme::SMALL_CORNER_RADIUS,
        theme::PROPERTY_HEAD_ICON_BG,
    );
    icon.image(IconSize::Small)
        .tint(theme::PROPERTY_HEAD_ICON)
        .paint_at(
            ui,
            Rect::from_center_size(boxed.center(), Vec2::splat(IconSize::Small.points())),
        );
    let left = boxed.right() + theme::PROPERTY_HEAD_GAP;
    let right = rect.right() - pad();
    let top = rect.top() + theme::PROPERTY_HEAD_PADDING_TOP;
    let line =
        |at: f32| Rect::from_min_max(pos2(left, at), pos2(right, at + theme::BODY_LINE_HEIGHT));
    // Painted, not labelled: the document's name is its tab's, and a feature's its Name
    // field's, so the accessibility tree has each once.
    for (at, mut job) in [
        (
            top,
            LayoutJob::single_section(
                title.to_owned(),
                format(theme::TITLE_SIZE, theme::PROPERTY_HEAD_TITLE, false),
            ),
        ),
        (top + theme::BODY_LINE_HEIGHT, sub),
    ] {
        let line = line(at);
        job.wrap = TextWrapping::truncate_at_width(line.width());
        let galley = ui.painter().layout_job(job);
        let at = pos2(line.left(), line.center().y - galley.size().y / 2.0);
        ui.painter().galley(at, galley, theme::PROPERTY_HEAD_TITLE);
    }
}

/// A section's header (`.section`): `text` in capitals, and the rule after it.
pub(crate) fn section(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            theme::PROPERTY_SECTION_PADDING_TOP
                + theme::BODY_LINE_HEIGHT
                + theme::PROPERTY_SECTION_PADDING_BOTTOM,
        ),
        Sense::hover(),
    );
    let top = rect.top() + theme::PROPERTY_SECTION_PADDING_TOP;
    let line = Rect::from_min_max(
        pos2(rect.left() + pad(), top),
        pos2(rect.right() - pad(), top + theme::BODY_LINE_HEIGHT),
    );
    let job = LayoutJob::single_section(
        text.to_uppercase(),
        TextFormat {
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            ..format(
                theme::PROPERTY_SECTION_SIZE,
                theme::PROPERTY_SECTION_TEXT,
                false,
            )
        },
    );
    let words = within(ui, line, ("section", text), |ui| {
        ui.add(Label::new(job).selectable(false))
    })
    .rect;
    let from = words.right() + theme::PROPERTY_SECTION_GAP;
    if from < line.right() {
        ui.painter().hline(
            from..=line.right(),
            line.center().y,
            Stroke::new(theme::BORDER_WIDTH, theme::PROPERTY_SECTION_RULE),
        );
    }
}

/// A property's row (`.prop`): `key` in its column, and what `value` adds in the rest, given
/// the key's label's ID (an input's `labelled_by`).
pub(crate) fn row<R>(ui: &mut Ui, key: &str, value: impl FnOnce(&mut Ui, egui::Id) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::PROPERTY_ROW_HEIGHT),
        Sense::hover(),
    );
    let key_rect = Rect::from_min_size(
        pos2(rect.left() + pad(), rect.top()),
        vec2(theme::PROPERTY_KEY_WIDTH, rect.height()),
    );
    let value_rect = Rect::from_min_max(
        pos2(key_rect.right() + theme::PROPERTY_ROW_GAP, rect.top()),
        pos2(rect.right() - pad(), rect.bottom()),
    );
    let key_text = key;
    let key = within(ui, key_rect, ("property-key", key_text), |ui| {
        truncated(ui, body(key_text, theme::PROPERTY_KEY))
    });
    // An ID of the key's own, so a field's focus stays put as the rows above it change.
    within(ui, value_rect, ("property", key_text), |ui| {
        value(ui, key.id)
    })
}

/// A row whose value is `value`'s text, cut short where it doesn't fit.
pub(crate) fn text_row(ui: &mut Ui, key: &str, value: LayoutJob) {
    row(ui, key, |ui, _| truncated(ui, value));
}

/// A row of a modal's list (`.prop` with `padding: 0`, as the prototype's "What changed on
/// disk" lists them): `key` in its column, in `key_colour` (the property key's, or purple
/// for a feature another program changed), and `value` in the rest, each cut short where
/// it doesn't fit; `list`, naming the list among the modal's, and `at`, its place among
/// the list's rows, naming it.
pub(crate) fn list_row(
    ui: &mut Ui,
    (list, at): (&str, usize),
    key: &str,
    key_colour: Color32,
    value: &str,
) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::PROPERTY_ROW_HEIGHT),
        Sense::hover(),
    );
    let key_rect = Rect::from_min_size(
        rect.left_top(),
        vec2(theme::PROPERTY_KEY_WIDTH, rect.height()),
    );
    let value_rect = Rect::from_min_max(
        pos2(key_rect.right() + theme::PROPERTY_ROW_GAP, rect.top()),
        rect.right_bottom(),
    );
    within(ui, key_rect, ("list-key", list, at), |ui| {
        truncated(ui, body(key, key_colour))
    });
    within(ui, value_rect, ("list-value", list, at), |ui| {
        truncated(ui, body(value, theme::PROPERTY_VALUE))
    });
}

/// A hint (`.hint-block`): each of `parts` in turn, the emphasised ones Gray.800.
pub(crate) fn hint(ui: &mut Ui, parts: &[(&str, bool)]) {
    hint_at(
        ui,
        parts,
        Margin {
            left: theme::VIEW_PADDING,
            right: theme::VIEW_PADDING,
            top: theme::HINT_MARGIN as i8,
            bottom: theme::HINT_MARGIN as i8,
        },
    );
}

/// [`hint`], `margin` around it: a modal's (`margin: 12px 0 0`), or a panel's.
pub(crate) fn hint_at(ui: &mut Ui, parts: &[(&str, bool)], margin: Margin) {
    let mut job = LayoutJob::default();
    for (text, strong) in parts {
        let colour = if *strong {
            theme::HINT_STRONG
        } else {
            theme::HINT_TEXT
        };
        job.append(
            text,
            0.0,
            TextFormat {
                line_height: Some(theme::BODY_LINE_HEIGHT),
                ..format(theme::HEADER_SIZE, colour, false)
            },
        );
    }
    egui::Frame::new()
        .fill(theme::HINT_BG)
        .corner_radius(CornerRadius::same(theme::NORMAL_CORNER_RADIUS))
        .inner_margin(theme::HINT_PADDING)
        .outer_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(Label::new(job).wrap().selectable(false));
        });
}

/// The card saying another program changed the selected feature (`featureProps`' purple
/// `.err-card.warn`): its head, the diff icon and "Changed on disk"; what changed, `text`;
/// and Mark as seen (`.btn-ghost`), which takes its mark away. Whether that was clicked.
pub(crate) fn outside_card(ui: &mut Ui, text: &str) -> bool {
    let words = |text: &str, colour| {
        let mut job = LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                line_height: Some(theme::BODY_LINE_HEIGHT),
                ..format(theme::BODY_SIZE, colour, false)
            },
        );
        job.wrap.max_width = f32::INFINITY;
        job
    };
    let mut seen = false;
    egui::Frame::new()
        .fill(theme::OUTSIDE_CARD_BG)
        .stroke(Stroke::new(theme::BORDER_WIDTH, theme::OUTSIDE_CARD_BORDER))
        .corner_radius(CornerRadius::same(theme::NORMAL_CORNER_RADIUS))
        .outer_margin(Margin {
            left: theme::VIEW_PADDING,
            right: theme::VIEW_PADDING,
            top: theme::ERROR_CARD_MARGIN as i8,
            bottom: theme::ERROR_CARD_MARGIN as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(theme::ERROR_CARD_HEAD_PADDING)
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = vec2(theme::ERROR_CARD_GAP, 0.0);
                        let side = IconSize::Small.points();
                        let (at, _) = ui.allocate_exact_size(
                            vec2(side, theme::BODY_LINE_HEIGHT),
                            Sense::hover(),
                        );
                        Icon::Diff
                            .image(IconSize::Small)
                            .tint(theme::OUTSIDE_CARD_TEXT)
                            .paint_at(ui, Rect::from_min_size(at.min, Vec2::splat(side)));
                        ui.add(
                            Label::new(words("Changed on disk", theme::TEXT_STRONG))
                                .selectable(false),
                        );
                    });
                });
            egui::Frame::new()
                .inner_margin(theme::ERROR_CARD_BODY_PADDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        Label::new(words(text, theme::OUTSIDE_CARD_TEXT))
                            .wrap()
                            .selectable(false),
                    );
                    // `.ec-body p { margin: 0 0 6px }`, then the actions.
                    ui.add_space(theme::ERROR_CARD_FIX_GAP);
                    let button = modal::Button::new("Mark as seen", Kind::Ghost);
                    let (at, _) = ui.allocate_exact_size(
                        vec2(modal::natural_width(ui, &button), theme::BUTTON_HEIGHT),
                        Sense::hover(),
                    );
                    seen = modal::paint(ui, at, &button).clicked();
                });
        });
    seen
}

/// What an error card offers beside Copy as JSON (app.js's `errCard` and its `noEdit` and
/// `noShow`): Edit, if it's offered, and whether the feature's dialog can open now; and
/// Show in timeline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Offer {
    pub edit: Option<bool>,
    pub show: bool,
}

/// What an error card's button asked: the feature's dialog opened (Edit), or the feature
/// selected and the timeline scrolled to it (Show in timeline). Copy as JSON is done where
/// it's clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardAct {
    Edit(FeatureId),
    Show(FeatureId),
}

/// The card's colours: an error's, or a warning's (`.err-card.warn`).
struct CardLook {
    fill: Color32,
    border: Color32,
    head: Color32,
    words: Color32,
    icon: Icon,
}

impl CardLook {
    fn of(warning: bool) -> Self {
        if warning {
            CardLook {
                fill: theme::WARNING_CARD_BG,
                border: theme::WARNING_CARD_BORDER,
                head: theme::WARNING_CARD_HEAD,
                words: theme::WARNING_CARD_BODY_TEXT,
                icon: Icon::Warn,
            }
        } else {
            CardLook {
                fill: theme::ERROR_CARD_BG,
                border: theme::ERROR_CARD_BORDER,
                head: theme::ERROR_CARD_HEAD,
                words: theme::ERROR_CARD_BODY_TEXT,
                icon: Icon::Error,
            }
        }
    }
}

/// An error card (`.err-card`; app.js's `errCard`; DESIGN.md, *Errors carry the agent's
/// words*), a warning's in its own colours if `said` is a warning's: its icon, its title
/// (`.ec-head .t`, [`card_title`]), and its code at the head's right (`.ec-code`); under
/// them the core's message (`.ec-body p`) where the title isn't it, its cause, its Feature
/// (`name`, if the timeline lists it, then its ID), Elements (each a chip, and how many in
/// all past those listed), and Parameter rows (`.ec-kv`), its suggestion after an arrow
/// (`.ec-fix`), the names it offers in place of a broken one, and its buttons
/// (`.ec-actions`): Edit and Show in timeline as `offer` offers them, and Copy as JSON, the
/// core's error as an agent gets it. Each part only where `said` has it: a refusal said in
/// words alone is its message and suggestion. What a button asked, if one was.
///
/// SEAM: each candidate's Use and the card's Remove the edge (a fillet's or chamfer's
/// `edges` written with the candidate in the broken name's place, or without it), once the
/// card can send an edit; and each candidate's why (`.cand .why`), which the core doesn't
/// give.
pub(crate) fn card(ui: &mut Ui, said: &Said, name: Option<&str>, offer: Offer) -> Option<CardAct> {
    let look = CardLook::of(said.warning);
    let (title, message) = card_title(said, name);
    let mut asked = None;
    egui::Frame::new()
        .fill(look.fill)
        .stroke(Stroke::new(theme::BORDER_WIDTH, look.border))
        .corner_radius(CornerRadius::same(theme::NORMAL_CORNER_RADIUS))
        .outer_margin(Margin {
            left: theme::VIEW_PADDING,
            right: theme::VIEW_PADDING,
            top: theme::ERROR_CARD_MARGIN as i8,
            bottom: theme::ERROR_CARD_MARGIN as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            egui::Frame::new()
                .inner_margin(theme::ERROR_CARD_HEAD_PADDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    card_head(ui, said, title, &look);
                });
            egui::Frame::new()
                .inner_margin(theme::ERROR_CARD_BODY_PADDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    asked = card_body(ui, said, message, name, offer, &look);
                });
        });
    asked
}

/// A card for each of `listed`, of `tab`'s document (`errCard(p)`, as `docProps` and the
/// `whatswrong` modal list them): each by the name of the feature it names, with Edit
/// (enabled where its dialog can open now) and Show in timeline where the timeline lists
/// that feature, and Copy as JSON; each card's widgets under `salt` and its place. What a
/// card's button asked, if one was.
pub(crate) fn document_cards(
    ui: &mut Ui,
    tab: &crate::tab::Tab,
    listed: &[Said],
    salt: &str,
) -> Option<CardAct> {
    let mut asked = None;
    for (place, said) in listed.iter().enumerate() {
        let entry = said.feature().and_then(|id| tab.entry(id));
        let offer = Offer {
            edit: Some(entry.is_some_and(|entry| tab.can_edit(&entry.id))),
            show: entry.is_some(),
        };
        let name = entry.map(|entry| entry.name.as_str());
        let card = ui
            .push_id((salt, place), |ui| card(ui, said, name, offer))
            .inner;
        asked = asked.or(card);
    }
    asked
}

/// `text` in the body's type on a line of the body's, in `colour`, unwrapped (a label
/// wraps it).
fn card_words(text: &str, colour: Color32) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            line_height: Some(theme::BODY_LINE_HEIGHT),
            ..format(theme::BODY_SIZE, colour, false)
        },
    );
    job.wrap.max_width = f32::INFINITY;
    job
}

/// A card's title (`.ec-head .t`, the prototype's `pr.title`): the name of the feature
/// `said` names (`name`, if the timeline lists it, else its ID, as the Feature row falls
/// back), its message below it in the body. The core gives no title, so a card that names
/// no feature (the gate's own problems, a selection's refusal) keeps the core's message
/// here, and its body doesn't say it again. The title, and the message for the body if
/// the title isn't it.
fn card_title<'a>(said: &'a Said, name: Option<&'a str>) -> (&'a str, Option<&'a str>) {
    match said.feature() {
        Some(id) => (name.unwrap_or(id), Some(&said.message)),
        None => (&said.message, None),
    }
}

/// The head (`.ec-head`, `align-items: flex-start`): the icon at the top of the first line,
/// `title` in white beside it (`.t { flex: 1 }`), wrapped short of the code chip at the
/// head's right (`white-space: nowrap`).
fn card_head(ui: &mut Ui, said: &Said, title: &str, look: &CardLook) {
    let width = ui.available_width();
    let side = IconSize::Small.points();
    let code = said.code().map(|code| {
        ui.painter().layout_no_wrap(
            code.to_owned(),
            FontId::monospace(theme::ERROR_CARD_CODE_SIZE),
            theme::ERROR_CARD_ACCENT,
        )
    });
    let chip_width = code
        .as_ref()
        .map(|code| code.size().x + 2.0 * theme::ERROR_CARD_CODE_PADDING);
    let room = chip_width.map_or(0.0, |chip| chip + theme::ERROR_CARD_GAP);
    let mut job = card_words(title, theme::TEXT_STRONG);
    job.wrap.max_width = (width - side - theme::ERROR_CARD_GAP - room).max(1.0);
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, galley.size().y.max(theme::BODY_LINE_HEIGHT)),
        Sense::hover(),
    );
    look.icon
        .image(IconSize::Small)
        .tint(look.head)
        .paint_at(ui, Rect::from_min_size(rect.min, Vec2::splat(side)));
    modal::label_at(
        ui,
        pos2(rect.left() + side + theme::ERROR_CARD_GAP, rect.top()),
        galley,
    );
    if let (Some(code), Some(chip_width)) = (code, chip_width) {
        let chip = Rect::from_min_size(
            pos2(rect.right() - chip_width, rect.top()),
            vec2(chip_width, theme::BODY_LINE_HEIGHT),
        );
        ui.painter().rect_filled(
            chip,
            theme::ERROR_CARD_CODE_CORNER_RADIUS,
            theme::ERROR_CARD_CODE_BG,
        );
        let text = code.text().to_owned();
        ui.painter().galley(
            pos2(
                chip.left() + theme::ERROR_CARD_CODE_PADDING,
                chip.center().y - code.size().y / 2.0,
            ),
            code,
            theme::ERROR_CARD_ACCENT,
        );
        let response = ui.interact(chip, ui.id().with("error-code"), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &text));
        response.on_hover_text("The error's code, as agents receive it");
    }
}

/// The body (`.ec-body`): what [`card`] says under its head, the core's `message` first
/// (`.ec-body p`) unless the head says it, and what its buttons asked.
fn card_body(
    ui: &mut Ui,
    said: &Said,
    message: Option<&str>,
    name: Option<&str>,
    offer: Offer,
    look: &CardLook,
) -> Option<CardAct> {
    for paragraph in message.into_iter().chain(said.cause.as_deref()) {
        ui.add(
            Label::new(card_words(paragraph, look.words))
                .wrap()
                .selectable(false),
        );
        ui.add_space(theme::ERROR_CARD_PARAGRAPH_MARGIN);
    }
    let rows = card_rows(said, name);
    if !rows.is_empty() {
        card_grid(ui, &rows, look);
        ui.add_space(theme::ERROR_CARD_ROWS_MARGIN);
    }
    if let Some(suggestion) = &said.suggestion {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = vec2(theme::ERROR_CARD_FIX_GAP, 0.0);
            ui.add(Label::new(card_words("→", theme::ERROR_CARD_ACCENT)).selectable(false));
            ui.add(
                Label::new(card_words(suggestion, theme::TEXT_STRONG))
                    .wrap()
                    .selectable(false),
            );
        });
        ui.add_space(theme::ERROR_CARD_FIX_MARGIN);
    }
    candidates(ui, said.candidates());
    card_buttons(ui, said, name, offer)
}

/// A row of the card's (`.ec-kv`): its key, what it says in the accessibility tree
/// ("Elements: Edge3, Edge5 (the first 2 of 5)"), and its value.
struct CardRow {
    key: &'static str,
    said: String,
    value: RowValue,
}

/// A row's value as it's drawn.
enum RowValue {
    /// The feature's name, if the timeline lists it, then its ID in the monospace font.
    Feature { name: Option<String>, id: String },
    /// Each element named, a chip each, then, if more are at fault than are named, how many
    /// in all.
    Elements {
        names: Vec<String>,
        more: Option<String>,
    },
    /// Words.
    Words(String),
}

/// The rows `said` has: its Feature (by `name` if the timeline lists it), its Elements,
/// and its Parameter, in the prototype's order.
fn card_rows(said: &Said, name: Option<&str>) -> Vec<CardRow> {
    let mut rows = Vec::new();
    if let Some(id) = said.feature() {
        rows.push(CardRow {
            key: "Feature",
            said: match name {
                Some(name) => format!("Feature: {name} {id}"),
                None => format!("Feature: {id}"),
            },
            value: RowValue::Feature {
                name: name.map(str::to_owned),
                id: id.to_owned(),
            },
        });
    }
    if let Some(line) = said.elements_line() {
        let named = said.elements.len() as u64;
        rows.push(CardRow {
            key: "Elements",
            said: line,
            value: RowValue::Elements {
                names: said.elements.clone(),
                more: (said.elements_total > named)
                    .then(|| format!("the first {named} of {}", said.elements_total)),
            },
        });
    }
    if let Some(parameter) = said.parameter() {
        rows.push(CardRow {
            key: "Parameter",
            said: format!("Parameter: {parameter}"),
            value: RowValue::Words(parameter.to_owned()),
        });
    }
    rows
}

/// An element's name as its chip shows it: whole up to
/// [`theme::ERROR_CARD_CHIP_LONGEST`] characters, else its first
/// [`theme::ERROR_CARD_CHIP_KEPT`] and an ellipsis (app.js's `e.slice(0, 32) + '…'`).
pub(crate) fn chip_text(name: &str) -> String {
    if name.chars().count() > theme::ERROR_CARD_CHIP_LONGEST {
        let kept: String = name.chars().take(theme::ERROR_CARD_CHIP_KEPT).collect();
        format!("{kept}…")
    } else {
        name.to_owned()
    }
}

/// The rows' grid (`.ec-kv`, `grid-template-columns: auto 1fr`): each key in the accent,
/// in a column as wide as the widest, and its value after the gap, the rows 2 apart; each
/// row one label in the accessibility tree, saying its key and its value.
fn card_grid(ui: &mut Ui, rows: &[CardRow], look: &CardLook) {
    let small = |text: &str, colour: Color32| {
        LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                line_height: Some(theme::BODY_LINE_HEIGHT),
                ..format(theme::ERROR_CARD_ROW_SIZE, colour, false)
            },
        )
    };
    let keys: Vec<_> = rows
        .iter()
        .map(|row| {
            ui.painter()
                .layout_job(small(row.key, theme::ERROR_CARD_ACCENT))
        })
        .collect();
    let column = keys.iter().map(|key| key.size().x).fold(0.0, f32::max);
    let width = ui.available_width();
    let value_left = column + theme::ERROR_CARD_ROW_GAP;
    let value_width = (width - value_left).max(1.0);
    for (at, (row, key)) in rows.iter().zip(keys).enumerate() {
        if at > 0 {
            ui.add_space(theme::ERROR_CARD_ROW_SPACING);
        }
        let painter = ui.painter().clone();
        let place = ui.cursor().min;
        let height = match &row.value {
            RowValue::Feature { name, id } => {
                let mut job = LayoutJob::default();
                if let Some(name) = name {
                    job.append(
                        &format!("{name} "),
                        0.0,
                        TextFormat {
                            line_height: Some(theme::BODY_LINE_HEIGHT),
                            ..format(theme::ERROR_CARD_ROW_SIZE, look.words, false)
                        },
                    );
                }
                job.append(
                    id,
                    0.0,
                    TextFormat {
                        line_height: Some(theme::BODY_LINE_HEIGHT),
                        ..format(
                            theme::MONOSPACE_SIZE,
                            look.words.gamma_multiply(theme::ERROR_CARD_ID_OPACITY),
                            true,
                        )
                    },
                );
                job.wrap.max_width = value_width;
                let galley = painter.layout_job(job);
                let height = galley.size().y;
                painter.galley(place + vec2(value_left, 0.0), galley, look.words);
                height
            }
            RowValue::Words(words) => {
                let mut job = small(words, look.words);
                job.wrap.max_width = value_width;
                let galley = painter.layout_job(job);
                let height = galley.size().y;
                painter.galley(place + vec2(value_left, 0.0), galley, look.words);
                height
            }
            RowValue::Elements { names, more } => element_chips(
                ui,
                place + vec2(value_left, 0.0),
                value_width,
                names,
                more.as_deref(),
                look,
            ),
        };
        let height = height.max(theme::BODY_LINE_HEIGHT);
        painter.galley(place, key, theme::ERROR_CARD_ACCENT);
        let (_, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &row.said));
    }
}

/// The Elements row's value, from `at`, `width` wide: each of `names` a chip (`.el-chip`),
/// a space apart, wrapping as words do, then `more` in the row's words at the ID's opacity.
/// Its height.
fn element_chips(
    ui: &Ui,
    at: egui::Pos2,
    width: f32,
    names: &[String],
    more: Option<&str>,
    look: &CardLook,
) -> f32 {
    let painter = ui.painter();
    let line = theme::BODY_LINE_HEIGHT;
    let space = painter
        .layout_no_wrap(
            " ".to_owned(),
            FontId::proportional(theme::ERROR_CARD_ROW_SIZE),
            look.words,
        )
        .size()
        .x;
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut place = |item: f32| {
        if x > 0.0 && x + item > width {
            x = 0.0;
            y += line;
        }
        let left = x;
        x += item + space;
        pos2(at.x + left, at.y + y)
    };
    for name in names {
        let galley = painter.layout_no_wrap(
            chip_text(name),
            FontId::monospace(theme::ERROR_CARD_CHIP_SIZE),
            theme::ERROR_CARD_CHIP_TEXT,
        );
        let size = vec2(
            galley.size().x + 2.0 * theme::ERROR_CARD_CHIP_PADDING,
            galley.size().y,
        );
        let corner = place(size.x);
        let chip = Rect::from_min_size(pos2(corner.x, corner.y + (line - size.y) / 2.0), size);
        painter.rect_filled(
            chip,
            theme::ERROR_CARD_CHIP_CORNER_RADIUS,
            theme::ERROR_CARD_CHIP_BG,
        );
        painter.galley(
            pos2(chip.left() + theme::ERROR_CARD_CHIP_PADDING, chip.top()),
            galley,
            theme::ERROR_CARD_CHIP_TEXT,
        );
    }
    if let Some(more) = more {
        let colour = look.words.gamma_multiply(theme::ERROR_CARD_ID_OPACITY);
        let galley = painter.layout_job(LayoutJob::single_section(
            more.to_owned(),
            TextFormat {
                line_height: Some(line),
                ..format(theme::ERROR_CARD_ROW_SIZE, colour, false)
            },
        ));
        let corner = place(galley.size().x);
        painter.galley(corner, galley, colour);
    }
    y + line
}

/// The names a broken reference's error offers in its place, closest first (`errCard`'s
/// candidates): a heading in capitals, then each name in a row of its own (`.cand`), its
/// name in the monospace font, cut short with an ellipsis where it doesn't fit.
fn candidates(ui: &mut Ui, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let heading = LayoutJob::single_section(
        "CANDIDATES, CLOSEST FIRST".to_owned(),
        TextFormat {
            line_height: Some(theme::BODY_LINE_HEIGHT),
            extra_letter_spacing: theme::CAPITALS_LETTER_SPACING,
            ..format(
                theme::ERROR_CARD_CANDIDATES_SIZE,
                theme::ERROR_CARD_ACCENT,
                false,
            )
        },
    );
    ui.add(Label::new(heading).selectable(false));
    ui.add_space(theme::ERROR_CARD_CANDIDATES_MARGIN);
    for name in names {
        egui::Frame::new()
            .fill(theme::ERROR_CARD_CANDIDATE_BG)
            .corner_radius(CornerRadius::same(theme::SMALL_CORNER_RADIUS))
            .inner_margin(theme::ERROR_CARD_CANDIDATE_PADDING)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut job = LayoutJob::single_section(
                    name.clone(),
                    TextFormat {
                        line_height: Some(theme::BODY_LINE_HEIGHT),
                        ..format(
                            theme::MONOSPACE_SIZE,
                            theme::ERROR_CARD_CANDIDATE_TEXT,
                            true,
                        )
                    },
                );
                job.wrap = TextWrapping::truncate_at_width(ui.available_width());
                ui.add(Label::new(job).selectable(false))
                    .on_hover_text(name.as_str());
            });
        ui.add_space(theme::ERROR_CARD_CANDIDATE_MARGIN);
    }
}

/// One of the card's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Edit,
    Show,
    Copy,
}

/// The card's buttons (`.ec-actions`, wrapped, 6 apart each way): Edit (the edit icon and
/// `name`, as `.btn-secondary`) if offered and the card names a feature, off if its dialog
/// can't open now; Show in timeline (`.btn-ghost`) if offered and it names one; and Copy as
/// JSON (`.btn-ghost`) if the core said it, which puts its JSON on the clipboard. What
/// Edit or Show asked, if either was clicked.
fn card_buttons(ui: &mut Ui, said: &Said, name: Option<&str>, offer: Offer) -> Option<CardAct> {
    let feature = said.feature().and_then(FeatureId::new);
    let json = said.json();
    let edit_words = match name {
        Some(name) => format!("Edit {name}"),
        None => "Edit feature".to_owned(),
    };
    let mut buttons: Vec<(Which, modal::Button<'_>)> = Vec::new();
    if feature.is_some()
        && let Some(on) = offer.edit
    {
        buttons.push((
            Which::Edit,
            modal::Button::new(&edit_words, Kind::Secondary)
                .icon(Icon::Edit)
                .enabled(on),
        ));
    }
    if feature.is_some() && offer.show {
        buttons.push((
            Which::Show,
            modal::Button::new("Show in timeline", Kind::Ghost),
        ));
    }
    if json.is_some() {
        buttons.push((Which::Copy, modal::Button::new("Copy as JSON", Kind::Ghost)));
    }
    if buttons.is_empty() {
        return None;
    }
    // Placed as `flex-wrap` places them: in a line until the next doesn't fit.
    let width = ui.available_width();
    let gap = theme::ERROR_CARD_ACTIONS_GAP;
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut places = Vec::with_capacity(buttons.len());
    for (_, button) in &buttons {
        let wide = modal::natural_width(ui, button);
        if x > 0.0 && x + wide > width {
            x = 0.0;
            y += theme::BUTTON_HEIGHT + gap;
        }
        places.push((x, y, wide));
        x += wide + gap;
    }
    let (whole, _) = ui.allocate_exact_size(vec2(width, y + theme::BUTTON_HEIGHT), Sense::hover());
    let mut asked = None;
    for ((which, button), (x, y, wide)) in buttons.iter().zip(places) {
        let rect = Rect::from_min_size(whole.min + vec2(x, y), vec2(wide, theme::BUTTON_HEIGHT));
        let response = modal::paint(ui, rect, button);
        let clicked = match which {
            Which::Copy => response
                .on_hover_text("Copies the structured error an agent receives")
                .clicked(),
            Which::Edit | Which::Show => response.clicked(),
        };
        if !clicked {
            continue;
        }
        match (which, &feature, &json) {
            (Which::Edit, Some(id), _) => asked = Some(CardAct::Edit(id.clone())),
            (Which::Show, Some(id), _) => asked = Some(CardAct::Show(id.clone())),
            (Which::Copy, _, Some(json)) => ui.ctx().copy_text(json.clone()),
            _ => {}
        }
    }
    asked
}

/// A selection box (`.selbox`), the active one if `active`: a chip of `chosen`'s name with
/// `icon`, or `empty` in Gray.550 (`.subdued`) if nothing is chosen.
pub(crate) fn selection_box(
    ui: &mut Ui,
    active: bool,
    icon: Icon,
    chosen: Option<&str>,
    empty: &str,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::FIELD_HEIGHT),
        Sense::hover(),
    );
    let (fill, border) = if active {
        (
            theme::SELECTION_BOX_ACTIVE_BG,
            theme::SELECTION_BOX_ACTIVE_BORDER,
        )
    } else {
        (theme::SELECTION_BOX_BG, Color32::TRANSPARENT)
    };
    ui.painter()
        .rect_filled(rect, theme::SMALL_CORNER_RADIUS, fill);
    if border != Color32::TRANSPARENT {
        // `border-left: 2px`, inside the box's rounded corner.
        let edge = Rect::from_min_size(rect.min, vec2(theme::SELECTION_BOX_BORDER, rect.height()));
        ui.painter().rect_filled(
            edge,
            CornerRadius {
                nw: theme::SMALL_CORNER_RADIUS,
                sw: theme::SMALL_CORNER_RADIUS,
                ne: 0,
                se: 0,
            },
            border,
        );
    }
    let inner = Rect::from_min_max(
        pos2(
            rect.left() + theme::SELECTION_BOX_BORDER + theme::SELECTION_BOX_PADDING_LEFT,
            rect.top(),
        ),
        pos2(
            rect.right() - theme::SELECTION_BOX_PADDING_RIGHT,
            rect.bottom(),
        ),
    );
    within(ui, inner, "selection", |ui| match chosen {
        Some(name) => chip(ui, icon, name),
        None => truncated(ui, body(empty, theme::TEXT_SUBDUED)),
    });
    response
}

/// A row under a selection box listing one of its items: `name`'s chip with `icon`, across
/// the row from the panel's padding (the value's column is too narrow for an element's
/// name, which a chip would cut short), its whole name its tooltip; and at the row's right
/// a button that removes it (`renderDialog`'s Clear, an `.ab-btn` 18 points square with a
/// ×), labelled "Remove `name`": whether it was clicked. Off unless `enabled`. `salt` names
/// the row among its siblings.
pub(crate) fn listed(
    ui: &mut Ui,
    salt: impl egui::AsIdSalt,
    icon: Icon,
    name: &str,
    enabled: bool,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::PROPERTY_ROW_HEIGHT),
        Sense::hover(),
    );
    let id = ui.id().with(salt);
    let value = Rect::from_min_max(
        pos2(rect.left() + pad(), rect.top()),
        pos2(rect.right() - pad(), rect.bottom()),
    );
    let side = theme::SELECTION_BOX_CLEAR_SIDE;
    let button = Rect::from_center_size(
        pos2(value.right() - side / 2.0, value.center().y),
        Vec2::splat(side),
    );
    let chip_at = Rect::from_min_max(
        value.min,
        pos2(button.left() - theme::CHIP_GAP, value.bottom()),
    );
    within(ui, chip_at, id.with("chip"), |ui| {
        chip(ui, icon, name).on_hover_text(name)
    });
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(button, id.with("remove"), sense);
    let label = format!("Remove {name}");
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &label));
    let ink = if response.hovered() && enabled {
        ui.painter().rect_filled(
            button,
            theme::SMALL_CORNER_RADIUS,
            theme::APP_BAR_BUTTON_HOVER_BG,
        );
        theme::TEXT_STRONG
    } else {
        theme::TEXT_DEFAULT
    };
    Icon::Close.image(IconSize::Small).tint(ink).paint_at(
        ui,
        Rect::from_center_size(button.center(), Vec2::splat(IconSize::Small.points())),
    );
    enabled && response.on_hover_text(label).clicked()
}

/// What a selection box says under it (`renderDialog`'s `.err-text`, 11 px, in a row whose
/// key is empty): `text`, wrapped, in `ink` ([`theme::SELECTION_BOX_SAID`] for a refusal,
/// [`theme::SELECTION_BOX_NOTE`] for a note).
pub(crate) fn said(ui: &mut Ui, text: &str, ink: Color32) {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            line_height: Some(theme::BODY_LINE_HEIGHT),
            ..format(theme::HEADER_SIZE, ink, false)
        },
    );
    let left = pad() + theme::PROPERTY_KEY_WIDTH + theme::PROPERTY_ROW_GAP;
    egui::Frame::new()
        .outer_margin(Margin {
            left: left as i8,
            right: theme::VIEW_PADDING,
            top: 0,
            bottom: theme::HINT_MARGIN as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(Label::new(job).wrap().selectable(false));
        });
}

/// A chip (`.selbox .chip`): `icon` and `name`, labelled `name`.
fn chip(ui: &mut Ui, icon: Icon, name: &str) -> Response {
    let side = IconSize::Small.points();
    let room = ui.available_width() - 2.0 * theme::CHIP_PADDING - side - theme::CHIP_GAP;
    let mut job = LayoutJob::single_section(
        name.to_owned(),
        format(theme::HEADER_SIZE, theme::CHIP_TEXT, false),
    );
    job.wrap = TextWrapping::truncate_at_width(room.max(0.0));
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(
        vec2(
            2.0 * theme::CHIP_PADDING + side + theme::CHIP_GAP + galley.size().x,
            theme::CHIP_HEIGHT,
        ),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, name));
    ui.painter()
        .rect_filled(rect, theme::CHIP_CORNER_RADIUS, theme::CHIP_BG);
    let left = rect.left() + theme::CHIP_PADDING;
    icon.image(IconSize::Small).tint(theme::CHIP_TEXT).paint_at(
        ui,
        Rect::from_min_size(pos2(left, rect.center().y - side / 2.0), Vec2::splat(side)),
    );
    ui.painter().galley(
        pos2(
            left + side + theme::CHIP_GAP,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        theme::CHIP_TEXT,
    );
    response
}

/// A choice among `options`, the `chosen` one shown, labelled `key`: `.seg` if each of its
/// words fits its share of `ui`'s width ([`fits_segmented`]), else a drop-down
/// (`.field.select`, as `renderDialog`'s Extent); the one picked, if one was. Off unless
/// `enabled`.
pub(crate) fn choice(
    ui: &mut Ui,
    key: &str,
    options: &[String],
    chosen: usize,
    enabled: bool,
) -> Option<usize> {
    if fits_segmented(ui, options, ui.available_width()) {
        segmented(ui, options, chosen, enabled)
    } else {
        select(ui, key, options, chosen, enabled)
    }
}

/// Whether each of `options`' words, as `.seg` draws them, fits its share of a `.seg`
/// `width` wide with [`theme::SEGMENTED_ROOM`] at each side.
pub(crate) fn fits_segmented(ui: &Ui, options: &[String], width: f32) -> bool {
    let count = options.len().max(1) as f32;
    let inner = width - 2.0 * theme::SEGMENTED_PADDING;
    let share = (inner - (count - 1.0) * theme::SEGMENTED_PADDING) / count;
    options.iter().all(|shown| {
        let words = ui
            .painter()
            .layout_job(body(shown, theme::SEGMENTED_TEXT))
            .size()
            .x;
        words + 2.0 * theme::SEGMENTED_ROOM <= share
    })
}

/// A drop-down (`.field.select`) labelled `key`, showing `options`' `chosen` one, a menu of
/// them opened by a click: the one picked, if one was.
fn select(
    ui: &mut Ui,
    key: &str,
    options: &[String],
    chosen: usize,
    enabled: bool,
) -> Option<usize> {
    let shown = options.get(chosen).cloned().unwrap_or_default();
    let at = field_rect(ui, ui.available_width());
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.put(at, |ui: &mut Ui| {
                let (rect, response) = ui.allocate_exact_size(at.size(), Sense::click());
                let enabled = ui.is_enabled();
                response.widget_info(|| WidgetInfo {
                    label: Some(key.to_owned()),
                    current_text_value: Some(shown.clone()),
                    enabled,
                    ..WidgetInfo::new(WidgetType::ComboBox)
                });
                let fill = if enabled && response.hovered() {
                    theme::FIELD_HOVER_BG
                } else {
                    theme::FIELD_BG
                };
                ui.painter()
                    .rect_filled(rect, theme::SMALL_CORNER_RADIUS, fill);
                theme::focus_ring(
                    ui,
                    &response,
                    rect,
                    theme::SMALL_CORNER_RADIUS,
                    StrokeKind::Inside,
                );
                let inner = rect.shrink2(vec2(theme::BORDER_WIDTH + theme::SELECT_PADDING, 0.0));
                let arrow = Rect::from_min_size(
                    pos2(
                        inner.right() - theme::SELECT_ARROW_SIZE,
                        rect.center().y - theme::SELECT_ARROW_SIZE / 2.0,
                    ),
                    Vec2::splat(theme::SELECT_ARROW_SIZE),
                );
                Icon::Down
                    .image(IconSize::Small)
                    .tint(theme::SELECT_ARROW)
                    .paint_at(ui, arrow);
                let mut job = body(&shown, theme::SELECT_TEXT);
                job.wrap = TextWrapping::truncate_at_width(
                    (arrow.left() - theme::CHIP_GAP - inner.left()).max(0.0),
                );
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(
                    pos2(inner.left(), rect.center().y - galley.size().y / 2.0),
                    galley,
                    theme::SELECT_TEXT,
                );
                response
            })
        })
        .inner;
    let mut picked = None;
    crate::bar::menu(&response, |ui| {
        ui.set_min_width(at.width());
        for (place, option) in options.iter().enumerate() {
            if ui
                .add(egui::Button::selectable(place == chosen, option))
                .clicked()
            {
                picked = Some(place);
                ui.close();
            }
        }
    });
    picked
}

/// What a dialog's preview line says (`.dlg-status`, state 3e).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// Its rebuild runs, as far as it's got: Esc stops it.
    Running(Option<(usize, usize)>),
    /// Its rebuild was stopped: Rebuild resumes it.
    Stopped,
    /// Built with errors: OK keeps the feature with them.
    Errors(u64),
    /// Built, at the revision.
    UpToDate(u64),
}

/// The preview's line (`.dlg-status`): its mark, its words, and, while it runs, what stops
/// it. `rebuild` names Rebuild's shortcut.
///
/// SEAM: the preview's time (`· 84 ms`), as the status bar's rebuild's: `status` gives none.
pub(crate) fn preview(ui: &mut Ui, preview: Preview, rebuild: &str) {
    let plural = |n: u64| if n == 1 { "" } else { "s" };
    let (mark, words, ink) = match preview {
        Preview::Running(progress) => (
            None,
            match progress {
                Some((done, steps)) => format!("Rebuilding the preview · {done} of {steps}"),
                None => "Rebuilding the preview".to_owned(),
            },
            theme::DIALOG_STATUS_RUNNING_TEXT,
        ),
        Preview::Stopped => (
            Some(Icon::Stop),
            format!("Preview stopped. Rebuild {rebuild} resumes it."),
            theme::DIALOG_STATUS_TEXT,
        ),
        Preview::Errors(errors) => (
            Some(Icon::Error),
            format!(
                "The preview has {errors} error{}. OK keeps the feature with them.",
                plural(errors)
            ),
            theme::ERROR_TEXT,
        ),
        Preview::UpToDate(revision) => (
            Some(Icon::Check),
            format!("Preview up to date · revision {revision}"),
            theme::DIALOG_STATUS_TEXT,
        ),
    };
    egui::Frame::new()
        .fill(theme::DIALOG_STATUS_BG)
        .corner_radius(CornerRadius::same(theme::SMALL_CORNER_RADIUS))
        .inner_margin(theme::DIALOG_STATUS_PADDING)
        .outer_margin(Margin {
            left: theme::VIEW_PADDING,
            right: theme::VIEW_PADDING,
            top: theme::DIALOG_STATUS_MARGIN as i8,
            bottom: 0,
        })
        .show(ui, |ui| {
            // `align-items: center`: the mark centred on the words, however many lines.
            let width = ui.available_width();
            let side = match mark {
                Some(_) => IconSize::Small.points(),
                None => theme::SPINNER_SIZE,
            };
            let small = |text: &str, colour| {
                LayoutJob::single_section(
                    text.to_owned(),
                    TextFormat {
                        line_height: Some(theme::BODY_LINE_HEIGHT),
                        ..format(theme::HEADER_SIZE, colour, false)
                    },
                )
            };
            let esc = matches!(preview, Preview::Running(_)).then(|| {
                ui.painter()
                    .layout_job(small("Esc stops it", theme::TEXT_SUBDUED))
            });
            let room = width
                - side
                - theme::DIALOG_STATUS_GAP
                - esc
                    .as_ref()
                    .map_or(0.0, |esc| esc.size().x + theme::DIALOG_STATUS_GAP);
            let mut job = small(&words, ink);
            job.wrap.max_width = room.max(0.0);
            let galley = ui.painter().layout_job(job);
            let height = galley.size().y.max(side);
            let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            let middle = rect.center().y;
            let at = Rect::from_min_size(pos2(rect.left(), middle - side / 2.0), Vec2::splat(side));
            match mark {
                Some(icon) => icon.image(IconSize::Small).tint(ink).paint_at(ui, at),
                None => crate::status::spinner(ui, at.center()),
            }
            let said = |ui: &mut Ui, galley: std::sync::Arc<egui::Galley>, left: f32, salt| {
                let place =
                    Rect::from_min_size(pos2(left, middle - galley.size().y / 2.0), galley.size());
                let text = galley.text().to_owned();
                let colour = galley
                    .job
                    .sections
                    .first()
                    .map_or(theme::DIALOG_STATUS_TEXT, |section| section.format.color);
                ui.painter().galley(place.min, galley, colour);
                ui.interact(place, ui.id().with(("preview", salt)), Sense::hover())
                    .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &text));
            };
            said(ui, galley, at.right() + theme::DIALOG_STATUS_GAP, 0);
            if let Some(esc) = esc {
                let left = rect.right() - esc.size().x;
                said(ui, esc, left, 1);
            }
        });
}

/// A choice of a few (`.seg`): each of `options`, the `chosen` one on, each a selectable
/// button labelled as it reads, sharing the width; the one clicked, if one was. Off unless
/// `enabled`.
pub(crate) fn segmented(
    ui: &mut Ui,
    options: &[String],
    chosen: usize,
    enabled: bool,
) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            theme::SEGMENTED_HEIGHT + 2.0 * theme::SEGMENTED_PADDING,
        ),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, theme::SMALL_CORNER_RADIUS, theme::SEGMENTED_BG);
    let inner = rect.shrink(theme::SEGMENTED_PADDING);
    let count = options.len().max(1) as f32;
    let width = (inner.width() - (count - 1.0) * theme::SEGMENTED_PADDING) / count;
    let mut clicked = None;
    for (place, shown) in options.iter().enumerate() {
        let at = Rect::from_min_size(
            pos2(
                inner.left() + place as f32 * (width + theme::SEGMENTED_PADDING),
                inner.top(),
            ),
            vec2(width, theme::SEGMENTED_HEIGHT),
        );
        let on = place == chosen;
        let response = ui
            .scope_builder(UiBuilder::new().max_rect(at), |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let (rect, response) = ui.allocate_exact_size(at.size(), Sense::click());
                    let enabled = ui.is_enabled();
                    response.widget_info(|| {
                        WidgetInfo::selected(WidgetType::SelectableLabel, enabled, on, shown)
                    });
                    let ink = if on {
                        ui.painter().rect_filled(
                            rect,
                            theme::SEGMENTED_CORNER_RADIUS,
                            theme::SEGMENTED_ON_BG,
                        );
                        theme::SEGMENTED_ON_TEXT
                    } else if response.hovered() {
                        theme::SEGMENTED_HOVER_TEXT
                    } else {
                        theme::SEGMENTED_TEXT
                    };
                    theme::focus_ring(
                        ui,
                        &response,
                        rect,
                        theme::SEGMENTED_CORNER_RADIUS,
                        StrokeKind::Outside,
                    );
                    let galley = ui.painter().layout_job(body(shown, ink));
                    ui.painter()
                        .galley(rect.center() - galley.size() / 2.0, galley, ink);
                    response
                })
                .inner
            })
            .inner;
        if response.clicked() {
            clicked = Some(place);
        }
    }
    clicked
}

/// A footer button (`.dlg-footer .btn`) in `rect`: `text`, labelled so, then `cap` as a key
/// cap (`.btn .kbd`), of `kind`; off unless `enabled`.
pub(crate) fn footer_button(
    ui: &mut Ui,
    rect: Rect,
    text: &str,
    cap: &str,
    kind: Kind,
    enabled: bool,
) -> Response {
    modal::paint(
        ui,
        rect,
        &modal::Button::new(text, kind).cap(cap).enabled(enabled),
    )
}

/// The footer's place at the bottom of `rect` (`.dlg-footer`): its whole, and its two
/// buttons', side by side, sharing its width.
pub(crate) fn footer_places(rect: Rect) -> (Rect, Rect, Rect) {
    let height =
        2.0 * theme::DIALOG_FOOTER_PADDING + theme::DIALOG_BUTTON_HEIGHT + theme::BORDER_WIDTH;
    let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - height), rect.max);
    let top = footer.top() + theme::BORDER_WIDTH + theme::DIALOG_FOOTER_PADDING;
    let left = footer.left() + pad();
    let width = (footer.width() - 2.0 * pad() - theme::DIALOG_FOOTER_GAP) / 2.0;
    let first = Rect::from_min_size(pos2(left, top), vec2(width, theme::DIALOG_BUTTON_HEIGHT));
    let second = first.translate(vec2(width + theme::DIALOG_FOOTER_GAP, 0.0));
    (footer, first, second)
}

/// The footer's fill and its border above it.
pub(crate) fn footer_paint(ui: &Ui, footer: Rect) {
    ui.painter().rect_filled(footer, 0, theme::PANEL_BG);
    ui.painter().hline(
        footer.x_range(),
        footer.top() + theme::BORDER_WIDTH / 2.0,
        Stroke::new(theme::BORDER_WIDTH, theme::DIALOG_FOOTER_BORDER),
    );
}

/// `unit`, painted inside the field at `field` (`.field .unit`): Gray.550, its padding and
/// the field's outline from its right, centred on its height.
pub(crate) fn unit(ui: &Ui, field: Rect, unit: &str) {
    if unit.is_empty() {
        return;
    }
    let galley = ui.painter().layout_job(body(unit, theme::FIELD_UNIT));
    let at = pos2(
        field.right() - theme::FOCUS_WIDTH - theme::FIELD_UNIT_PADDING - galley.size().x,
        field.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(at, galley, theme::FIELD_UNIT);
}

/// A value's unit inside its field (`.field .unit`), Gray.550: its width.
pub(crate) fn unit_width(ui: &Ui, unit: &str) -> f32 {
    if unit.is_empty() {
        return 0.0;
    }
    ui.painter()
        .layout_job(body(unit, theme::TEXT_SUBDUED))
        .size()
        .x
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::modal::tests::{
        at, at_opacity, cap_border, colour, fills_at, font, icon_at, kbd_border, near, near_rect,
        painted, rects, sized, text_shape, widget,
    };

    /// styles.css: `.dlg-footer .btn { flex: 1; height: 26px }`; `.btn { gap: 6px }`;
    /// `.btn .kbd { margin-left: 2px; opacity: .55 }`, its border and words the button's
    /// (`.btn-secondary`'s Gray.750); `.kbd { height: 16px }`.
    const BTN_GAP: f32 = 6.0;
    const KBD_MARGIN: f32 = 2.0;
    const KBD_H: f32 = 16.0;
    const KBD_OPACITY: f32 = 0.55;
    const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);

    #[test]
    fn a_footer_buttons_key_cap_is_8_after_its_words_and_bordered_2_at_its_bottom() {
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(150.0, 26.0));
        let mut h = Harness::builder()
            .with_size(Vec2::new(240.0, 90.0))
            .build_ui(move |ui| {
                footer_button(ui, place, "Cancel", "Esc", Kind::Secondary, true);
            });
        theme::apply(&h.ctx);
        h.run_steps(2);
        let button = widget(&h, Role::Button, "Cancel");
        let words = text_shape(&h, "Cancel");
        let boxed = rects(&h)
            .into_iter()
            .find(|shape| {
                shape.fill == Color32::TRANSPARENT
                    && near(shape.rect.height(), KBD_H)
                    && button.contains_rect(shape.rect)
            })
            .expect("Esc's box");
        assert!(boxed.stroke.is_empty(), "Esc's box: {:?}", boxed.stroke);
        assert!(
            near(
                boxed.rect.left(),
                words.pos.x + words.galley.size().x + BTN_GAP + KBD_MARGIN
            ),
            "Esc at {:?}, Cancel's words at {:?}, {:?} wide",
            boxed.rect,
            words.pos,
            words.galley.size()
        );
        assert!(near(boxed.rect.center().y, button.center().y));
        kbd_border(
            &cap_border(&h, boxed.rect),
            boxed.rect,
            at_opacity(G750, KBD_OPACITY),
            "Esc",
        );
    }

    /// `body`: `font: 12px/16px`; `--icon`; `--r`; `--pad`; `.btn`'s 22.
    const FS: f32 = 12.0;
    const LH: f32 = 16.0;
    const ICON: f32 = 14.0;
    const PAD: f32 = 12.0;
    const BTN_H: f32 = 22.0;
    /// `.prop`: `grid-template-columns: 96px 1fr; gap: 8px`, `--row-h` high, unpadded in a
    /// modal's list; `.prop .k` Gray.550, `.prop .v` Gray.800.
    const KEY_W: f32 = 96.0;
    const ROW_GAP: f32 = 8.0;
    const ROW_H: f32 = 24.0;
    const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);

    #[test]
    fn a_modals_list_row_is_drawn_as_prop_its_value_8_after_the_keys_96() {
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(300.0, 100.0));
        let purple = Color32::from_rgb(0xb7, 0x80, 0xff);
        let mut h = Harness::builder()
            .with_size(Vec2::new(400.0, 160.0))
            .build_ui(move |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                    list_row(ui, ("theirs", 0), "Drill", purple, "name · 5 → 3");
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(2);
        let key = text_shape(&h, "Drill");
        let value = text_shape(&h, "name · 5 → 3");
        let middle = place.top() + ROW_H / 2.0;
        at(
            key.pos,
            Pos2::new(place.left(), middle - key.galley.size().y / 2.0),
            "the key",
        );
        at(
            value.pos,
            Pos2::new(
                place.left() + KEY_W + ROW_GAP,
                middle - value.galley.size().y / 2.0,
            ),
            "the value",
        );
        font(&key, FS, "the key");
        font(&value, FS, "the value");
        assert_eq!(colour(&key), purple);
        assert_eq!(colour(&value), G800);
    }

    /// `renderDialog`'s Clear: `.ab-btn` 18 square (`style="height:18px;min-width:18px"`),
    /// `--r-sm` round; `.ab-btn:hover:not(:disabled)` Gray.300 behind Gray.1000.
    /// `.selbox .chip`: 16 high, Gray.300, `padding: 0 5px; gap: 4px`.
    const CLEAR: f32 = 18.0;
    const CHIP_H: f32 = 16.0;
    const CHIP_GAP: f32 = 4.0;
    const G300: Color32 = Color32::from_rgb(0x2c, 0x2b, 0x2b);
    const G1000: Color32 = Color32::WHITE;

    /// A box of names' row listing `name`, `enabled` or not, drawn in `place`, its Remove
    /// button hovered if `hover`.
    fn listed_in(place: Rect, name: &'static str, enabled: bool, hover: bool) -> Harness<'static> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(300.0, 120.0))
            .build_ui(move |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                    listed(ui, "row", Icon::SelEdge, name, enabled);
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(2);
        if hover {
            h.get_by_role_and_label(Role::Button, &format!("Remove {name}"))
                .hover();
            h.run_steps(2);
        }
        h
    }

    #[test]
    fn a_listed_names_row_runs_from_the_pad_its_remove_button_18_square_at_its_right() {
        // `.prop`'s `padding: 0 var(--pad)`, `--row-h` high: the row's chip from 12 in, the
        // button 18 square, centred in the row, its right 12 in from the row's; the chip,
        // a name too long for the row cut short, ends a chip's gap (4) before the button.
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(200.0, 60.0));
        let name = "k7f2qx3m.fillet.Edge111111111111111111111111111111111111";
        let h = listed_in(place, name, true, false);
        let button = widget(&h, Role::Button, &format!("Remove {name}"));
        sized(
            button,
            Rect::from_min_size(
                Pos2::new(
                    place.right() - PAD - CLEAR,
                    place.top() + (ROW_H - CLEAR) / 2.0,
                ),
                Vec2::splat(CLEAR),
            ),
            "the Remove button",
        );
        let chip = rects(&h)
            .into_iter()
            .find(|shape| shape.fill == G300 && near(shape.rect.height(), CHIP_H))
            .expect("the chip")
            .rect;
        assert!(
            near(chip.left(), place.left() + PAD),
            "the chip at {chip:?}"
        );
        // Cut to fit, it ends within a narrow digit's width (under 8) of where it may.
        let end = button.left() - CHIP_GAP;
        assert!(
            chip.right() <= end + egui::emath::GUI_ROUNDING && chip.right() > end - 8.0,
            "the chip at {chip:?}, the button at {button:?}"
        );
    }

    #[test]
    fn a_listed_names_remove_button_is_lit_when_hovered_only_if_its_on() {
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(200.0, 60.0));
        let name = "k7f2qx3m.box.Edge3";
        for enabled in [true, false] {
            let h = listed_in(place, name, enabled, true);
            let button = Rect::from_min_size(
                Pos2::new(
                    place.right() - PAD - CLEAR,
                    place.top() + (ROW_H - CLEAR) / 2.0,
                ),
                Vec2::splat(CLEAR),
            );
            let lit: Vec<Color32> = fills_at(&h, button)
                .into_iter()
                .map(|(fill, _)| fill)
                .collect();
            let tint = icon_at(
                &h,
                Rect::from_center_size(button.center(), Vec2::splat(ICON)),
            );
            if enabled {
                assert_eq!(lit, [G300], "hovered, on");
                assert_eq!(tint, Some(G1000), "hovered, on");
            } else {
                assert!(lit.is_empty(), "hovered, off: {lit:?}");
                assert!(
                    tint.is_some_and(|tint| tint != G1000),
                    "hovered, off: {tint:?}"
                );
            }
        }
    }

    #[test]
    fn what_a_box_of_names_says_is_in_the_value_column_11_px_on_16_px_lines() {
        // `<div class="prop"><span></span><span class="err-text" style="font-size:11px;
        // white-space:normal">`: in the value's column, 12 + 96 + 8 in from the panel's
        // left, wrapped, each line `body`'s 16 high.
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(260.0, 200.0));
        let text = "k7f2qx3m.box.Face1 is a face: a fillet takes edges, so select one of the \
                    edges of the body it's built on";
        let red = Color32::from_rgb(0xff, 0x38, 0x65);
        let mut h = Harness::builder()
            .with_size(Vec2::new(360.0, 260.0))
            .build_ui(move |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                    said(ui, text, red);
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(2);
        let said = text_shape(&h, text);
        at(
            said.pos,
            Pos2::new(place.left() + PAD + KEY_W + ROW_GAP, place.top()),
            "what's said",
        );
        font(&said, 11.0, "what's said");
        assert_eq!(colour(&said), red);
        let lines = said.galley.rows.len();
        assert!(lines > 1, "what's said didn't wrap");
        assert!(
            near(said.galley.size().y, lines as f32 * LH),
            "{lines} lines {} high",
            said.galley.size().y
        );
    }

    #[test]
    fn the_changed_on_disk_card_is_drawn_as_feature_props_purple_err_card() {
        // `featureProps`' card: `.err-card` (`margin: 8px var(--pad)`, 1 border, `--r`) in
        // `rgba(183,128,255,.12)` bordered #463375; `.ec-head` (`padding: 8px 10px 4px;
        // gap: 8px`): the diff icon in #e4d6ff, "Changed on disk" white; `.ec-body`
        // (`padding: 0 10px 8px 32px`), #e4d6ff: what changed (`p`, `margin: 0 0 6px`), then
        // Mark as seen (`.btn.btn-ghost`). Every line 16 high.
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(295.0, 300.0));
        let text = "Block · height · 10 → 11, and the radius of its fillet too, which wraps";
        let mut h = Harness::builder()
            .with_size(Vec2::new(400.0, 360.0))
            .build_ui(move |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                    outside_card(ui, text);
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(2);
        let ink = Color32::from_rgb(0xe4, 0xd6, 0xff);
        let top = place.top() + 8.0;
        let inner = Pos2::new(place.left() + PAD + 1.0, top + 1.0);
        let icon = Rect::from_min_size(inner + Vec2::new(10.0, 8.0), Vec2::splat(ICON));
        assert_eq!(icon_at(&h, icon), Some(ink), "the card's icon");
        let head = text_shape(&h, "Changed on disk");
        at(
            head.pos,
            Pos2::new(icon.right() + 8.0, icon.top()),
            "the head",
        );
        font(&head, FS, "the head");
        assert_eq!(colour(&head), Color32::WHITE);
        assert!(near(head.galley.size().y, LH), "the head's line");
        let said = text_shape(&h, text);
        at(
            said.pos,
            Pos2::new(inner.x + 32.0, icon.top() + LH + 4.0),
            "what changed",
        );
        font(&said, FS, "what changed");
        assert_eq!(colour(&said), ink);
        let lines = said.galley.rows.len();
        assert!(lines > 1, "what changed didn't wrap");
        assert!(near(said.galley.size().y, lines as f32 * LH), "its lines");
        let seen = widget(&h, Role::Button, "Mark as seen");
        assert!(near(seen.left(), said.pos.x), "{seen:?}");
        assert!(near(seen.top(), said.pos.y + said.galley.size().y + 6.0));
        assert!(near(seen.height(), BTN_H));
        let card = Rect::from_min_max(
            Pos2::new(place.left() + PAD, top),
            Pos2::new(place.right() - PAD, seen.bottom() + 8.0 + 1.0),
        );
        let fills = fills_at(&h, card);
        assert_eq!(
            fills,
            [(
                Color32::from_rgba_unmultiplied(183, 128, 255, 31),
                egui::CornerRadius::same(6)
            )],
            "the card"
        );
    }

    // ---- The error card's rows and buttons, against `errCard` and `.err-card`'s rules ----

    /// styles.css's `.err-card` and its parts.
    mod ec {
        use eframe::egui::Color32;

        /// `.err-card`: `margin: 8px var(--pad)`, `1px solid #5c001a`, on
        /// `--alert-red-bg: rgba(236, 0, 79, 0.18)` (255 × .18 = 45.9, 46).
        pub const MARGIN: f32 = 8.0;
        pub const BORDER: f32 = 1.0;
        pub const BG: Color32 = Color32::from_rgba_unmultiplied_const(236, 0, 79, 46);
        /// `.ec-head`: `padding: 8px 10px 4px`; `.ec-body`: `padding: 0 10px 8px 32px`,
        /// `#ffe2e5`; `.ec-body p { margin: 0 0 6px }`.
        pub const HEAD_TOP: f32 = 8.0;
        pub const SIDE: f32 = 10.0;
        pub const HEAD_BOTTOM: f32 = 4.0;
        pub const BODY_LEFT: f32 = 32.0;
        pub const BODY_BOTTOM: f32 = 8.0;
        pub const BODY_TEXT: Color32 = Color32::from_rgb(0xff, 0xe2, 0xe5);
        pub const P_MARGIN: f32 = 6.0;
        /// `.ec-kv`: `gap: 2px 8px; font-size: var(--fs-small); margin-bottom: 6px`, on
        /// `body`'s 16 px line.
        pub const ROW_GAP: f32 = 2.0;
        pub const COLUMN_GAP: f32 = 8.0;
        pub const KV_MARGIN: f32 = 6.0;
        pub const FS_SMALL: f32 = 11.0;
        /// `.mono { opacity: .7 }` (`the first N of M` as the ID's): 255 × .7 = 178.5, 179.
        pub const FADED_ALPHA: u8 = 179;
        /// `.el-chip`: `padding: 0 5px; border-radius: 3px; background: rgba(0,0,0,.35)`
        /// (255 × .35 = 89.25, 89); the chips `.join(' ')`ed.
        pub const CHIP_PADDING: f32 = 5.0;
        pub const CHIP_RADIUS: u8 = 3;
        pub const CHIP_BG: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, 89);
        /// `.ec-actions { display: flex; flex-wrap: wrap; gap: 6px }`.
        pub const ACTIONS_GAP: f32 = 6.0;
    }

    /// `said`'s card, by `name`, offering `offer`, drawn in `place`.
    fn card_in(
        place: Rect,
        said: Said,
        name: Option<&'static str>,
        offer: Offer,
    ) -> Harness<'static> {
        let mut h = Harness::builder()
            .with_size(Vec2::new(place.right() + 40.0, place.bottom() + 40.0))
            .build_ui(move |ui| {
                ui.scope_builder(UiBuilder::new().max_rect(place), |ui| {
                    card(ui, &said, name, offer);
                });
            });
        theme::apply(&h.ctx);
        h.run_steps(2);
        h
    }

    /// The card's body in `place` (`.ec-body`'s inside): its left and right, and its top
    /// under a title `title` high.
    fn ec_body(place: Rect, title: f32) -> (f32, f32, f32) {
        let head_top = place.top() + ec::MARGIN + ec::BORDER + ec::HEAD_TOP;
        (
            place.left() + PAD + ec::BORDER + ec::BODY_LEFT,
            place.right() - PAD - ec::BORDER - ec::SIDE,
            head_top + title + ec::HEAD_BOTTOM,
        )
    }

    /// Where `shown`'s first glyph sits: its baseline, in the window.
    fn baseline(shown: &egui::epaint::TextShape) -> f32 {
        let row = &shown.galley.rows[0];
        shown.pos.y + row.pos.y + row.row.glyphs[0].pos.y
    }

    /// A space at `.ec-kv`'s 11 px, as the font sets it.
    fn space<S>(h: &Harness<'_, S>) -> f32 {
        h.ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(
                    " ".to_owned(),
                    FontId::proportional(ec::FS_SMALL),
                    Color32::WHITE,
                )
                .size()
                .x
        })
    }

    /// The `.ec-kv` keys `keys` as painted, each from the body's left, 11 px on a 16 line in
    /// the accent: the left of the values' column, 8 after the widest.
    #[track_caller]
    fn values_left<S>(h: &Harness<'_, S>, keys: &[&str], body_left: f32) -> f32 {
        let mut column = 0.0f32;
        for key in keys {
            let shown = text_shape(h, key);
            font(&shown, ec::FS_SMALL, key);
            assert!(near(shown.galley.size().y, LH), "{key}'s line");
            assert!(near(shown.pos.x, body_left), "{key} at {:?}", shown.pos);
            assert_eq!(colour(&shown), Color32::from_rgb(0xff, 0x99, 0xa6), "{key}");
            column = column.max(shown.galley.size().x);
        }
        body_left + column + ec::COLUMN_GAP
    }

    /// Inline items laid out as CSS lays out words, from `left` to `right` on lines `line`
    /// apart: each `(rect, its line's top)` after the one before it, `gap` from it on its
    /// line, or, where it wouldn't fit there, first on the next, at `left` (the first on
    /// a line staying there however wide). How many lines they take.
    #[track_caller]
    fn flowed(
        items: &[(Rect, f32)],
        left: f32,
        right: f32,
        gap: f32,
        line: f32,
        what: &str,
    ) -> usize {
        let mut lines = 1;
        for pair in items.windows(2) {
            let ((before, before_line), (item, item_line)) = (pair[0], pair[1]);
            if near(item_line, before_line) {
                assert!(
                    near(item.left(), before.right() + gap),
                    "{what}: {item:?} not {gap} after {before:?}"
                );
                assert!(
                    item.right() <= right + 0.5,
                    "{what}: {item:?} past {right} on its line"
                );
            } else {
                lines += 1;
                assert!(
                    near(item_line, before_line + line),
                    "{what}: {item:?}'s line"
                );
                assert!(
                    near(item.left(), left),
                    "{what}: {item:?} not first on its line"
                );
                assert!(
                    before.right() + gap + item.width() > right,
                    "{what}: {item:?} wrapped, though it fit after {before:?} in {right}"
                );
            }
        }
        lines
    }

    /// The Elements chips of `names`, each as painted: its rect, and its line's top.
    #[track_caller]
    fn chips<S>(h: &Harness<'_, S>, names: &[&str]) -> Vec<(Rect, f32)> {
        names
            .iter()
            .map(|name| {
                let shown = text_shape(h, name);
                let size = shown.galley.size();
                let chip = Rect::from_min_size(
                    shown.pos - Vec2::new(ec::CHIP_PADDING, 0.0),
                    Vec2::new(size.x + 2.0 * ec::CHIP_PADDING, size.y),
                );
                assert_eq!(
                    fills_at(h, chip),
                    [(ec::CHIP_BG, egui::CornerRadius::same(ec::CHIP_RADIUS))],
                    "{name}'s chip"
                );
                // Centred on its 16 line.
                (chip, chip.center().y - LH / 2.0)
            })
            .collect()
    }

    /// The error card's place in a 296 wide properties panel (less its border).
    fn panel_place(height: f32) -> Rect {
        Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(295.0, height))
    }

    #[test]
    fn the_elements_chips_wrap_as_words_their_count_after_them_and_the_parameter_under_them() {
        // `.ec-kv`'s Elements: the chips `.join(' ')`ed in the value's column, wrapping as
        // words do, a name too long for the column first on its line however wide; then
        // "the first 8 of 12" at the ID's 70%; Parameter 2 under the last line.
        const LONG: &str = "k7f2qx3m.box.Edge12345678901234567";
        let names = [
            LONG, "Edge11", "Edge12", "Edge13", "Edge14", "Edge15", "Edge16", "Edge17",
        ];
        let said = Said::of(
            &silico_core::Error::new(silico_core::ErrorCode::ReferenceBroken, "Edges are gone")
                .with_feature("c0000001")
                .with_elements(names)
                .with_elements_total(12)
                .with_parameter("edges"),
        );
        let place = panel_place(400.0);
        let offer = Offer {
            edit: None,
            show: false,
        };
        let h = card_in(place, said, Some("Hole"), offer);
        let title = text_shape(&h, "Hole").galley.size().y;
        let (body_left, body_right, body_top) = ec_body(place, title);
        let message = text_shape(&h, "Edges are gone");
        at(message.pos, Pos2::new(body_left, body_top), "the message");
        assert_eq!(colour(&message), ec::BODY_TEXT);
        let rows_top = body_top + message.galley.size().y + ec::P_MARGIN;
        let values = values_left(&h, &["Feature", "Elements", "Parameter"], body_left);
        at(
            text_shape(&h, "Feature").pos,
            Pos2::new(body_left, rows_top),
            "Feature",
        );
        // The feature's name on the key's baseline (one line, one font and size); the ID
        // after it on that line, faded.
        let feature = text_shape(&h, "Hole c0000001");
        at(feature.pos, Pos2::new(values, rows_top), "the feature");
        assert!(near(feature.galley.size().y, LH), "the feature's line");
        assert!(
            near(baseline(&feature), baseline(&text_shape(&h, "Feature"))),
            "the name off the key's baseline"
        );
        let elements_top = rows_top + LH + ec::ROW_GAP;
        at(
            text_shape(&h, "Elements").pos,
            Pos2::new(body_left, elements_top),
            "Elements",
        );
        let mut items = chips(&h, &names);
        let first = items[0].0;
        assert!(
            first.width() > body_right - values,
            "{LONG}'s chip fits the column: the case is gone"
        );
        assert!(near(first.left(), values), "the first chip at {first:?}");
        assert!(near(items[0].1, elements_top), "the first chip's line");
        // The count, in the row's words faded as the ID is, on a 16 line.
        let more = text_shape(&h, "the first 8 of 12");
        font(&more, ec::FS_SMALL, "the count");
        assert!(near(more.galley.size().y, LH), "the count's line");
        assert_eq!(colour(&more).a(), ec::FADED_ALPHA);
        items.push((
            Rect::from_min_size(more.pos, more.galley.size()),
            more.pos.y,
        ));
        let lines = flowed(&items, values, body_right, space(&h), LH, "the elements");
        assert!(lines >= 3, "{lines} lines: the chips didn't wrap twice");
        // Parameter, 2 under the last line, its value on a 16 line.
        let last_line = items[items.len() - 1].1;
        let parameter_top = last_line + LH + ec::ROW_GAP;
        at(
            text_shape(&h, "Parameter").pos,
            Pos2::new(body_left, parameter_top),
            "Parameter",
        );
        let parameter = text_shape(&h, "edges");
        at(parameter.pos, Pos2::new(values, parameter_top), "edges");
        assert!(near(parameter.galley.size().y, LH), "edges' line");
        font(&parameter, ec::FS_SMALL, "edges");
    }

    #[test]
    fn a_chip_that_just_fits_after_another_stays_on_its_line() {
        // Two chips and the space between them exactly as wide as the value's column: on
        // one line (CSS breaks a line only where the next word overflows it).
        let names = ["Edge11", "Edge2"];
        let said = || {
            Said::of(
                &silico_core::Error::new(silico_core::ErrorCode::ReferenceBroken, "Gone")
                    .with_feature("c0000001")
                    .with_elements(names),
            )
        };
        let offer = Offer::default();
        // Measured in a panel wide enough for both.
        let wide = card_in(panel_place(300.0), said(), Some("Hole"), offer);
        let measured = chips(&wide, &names);
        let column = ["Feature", "Elements"]
            .iter()
            .map(|key| text_shape(&wide, key).galley.size().x)
            .fold(0.0f32, f32::max);
        let both = measured[0].0.width() + space(&wide) + measured[1].0.width();
        // The panel the two fill: the card's margins, border and body padding, the keys'
        // column and its gap, and the two.
        let width =
            both + column + ec::COLUMN_GAP + 2.0 * (PAD + ec::BORDER) + ec::BODY_LEFT + ec::SIDE;
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(width, 300.0));
        let h = card_in(place, said(), Some("Hole"), offer);
        let (body_left, body_right, _) = ec_body(place, LH);
        let values = values_left(&h, &["Feature", "Elements"], body_left);
        assert_eq!(body_right - values, both, "the column, the two exactly");
        let items = chips(&h, &names);
        assert_eq!(
            flowed(&items, values, body_right, space(&h), LH, "the chips"),
            1
        );
    }

    #[test]
    fn an_id_alone_is_on_the_rows_16_line_and_every_element_named_gives_no_count() {
        // A card for a feature the timeline doesn't list: its Feature row the ID alone
        // (`f ? f.name : pr.feature`), `.mono` on the row's 16 line (`body`'s line-height,
        // whatever the font's own); its one element named,
        // of one, so no count.
        let said = Said::of(
            &silico_core::Error::new(silico_core::ErrorCode::KernelFailed, "Fillet failed")
                .with_feature("c0000001")
                .with_elements(["Edge3"]),
        );
        let place = panel_place(300.0);
        let h = card_in(place, said, None, Offer::default());
        let ids: Vec<_> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(shown) if shown.galley.text() == "c0000001" => Some(shown),
                _ => None,
            })
            .collect();
        // The title (the ID, as the Feature row falls back) and the row's value.
        assert_eq!(ids.len(), 2, "the ID painted {} times", ids.len());
        let value = ids
            .iter()
            .find(|shown| {
                shown.galley.job.sections[0].format.font_id.family == egui::FontFamily::Monospace
            })
            .expect("the ID in `.mono`");
        assert!(near(value.galley.size().y, LH), "the ID's line");
        let texts: Vec<String> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(shown) => Some(shown.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            !texts.iter().any(|text| text.starts_with("the first")),
            "{texts:?}"
        );
    }

    /// The card's buttons, in order, as the accessibility tree has them: each rect and its
    /// line's top.
    fn buttons<S>(h: &Harness<'_, S>, labels: &[&str]) -> Vec<(Rect, f32)> {
        labels
            .iter()
            .map(|label| {
                let rect = widget(h, Role::Button, label);
                assert!(near(rect.height(), BTN_H), "{label}: {rect:?}");
                (rect, rect.top())
            })
            .collect()
    }

    #[test]
    fn the_cards_buttons_wrap_as_ec_actions_flex_wrap_and_the_card_ends_8_under_them() {
        // `docProps`' card: Edit Hole, Show in timeline, and Copy as JSON, too wide together
        // for the body, wrapping (`flex-wrap: wrap; gap: 6px`): a line 22 high, the next 6
        // under it; the card's frame to 8 under the last (`.ec-body`'s padding) and its
        // border. Then a name so long its Edit is wider than the body: first on its line.
        for name in [
            "Hole",
            "Hole through the whole block for the long mounting bolt",
        ] {
            let said = Said::of(
                &silico_core::Error::new(
                    silico_core::ErrorCode::InvalidInput,
                    "The tool names no feature",
                )
                .with_feature("c0000001"),
            );
            let place = panel_place(400.0);
            let offer = Offer {
                edit: Some(false),
                show: true,
            };
            let h = card_in(place, said, Some(name), offer);
            let titles: Vec<f32> = painted(&h)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Text(shown) if shown.galley.text() == name => {
                        Some(shown.galley.size().y)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(titles.len(), 1, "{name}'s title");
            let (body_left, body_right, body_top) = ec_body(place, titles[0]);
            let message = text_shape(&h, "The tool names no feature");
            let rows_top = body_top + message.galley.size().y + ec::P_MARGIN;
            let feature = text_shape(&h, &format!("{name} c0000001"));
            let actions_top = rows_top + feature.galley.size().y.max(LH) + ec::KV_MARGIN;
            let edit = format!("Edit {name}");
            let items = buttons(&h, &[&edit, "Show in timeline", "Copy as JSON"]);
            at(
                items[0].0.left_top(),
                Pos2::new(body_left, actions_top),
                &edit,
            );
            let lines = flowed(
                &items,
                body_left,
                body_right,
                ec::ACTIONS_GAP,
                BTN_H + ec::ACTIONS_GAP,
                name,
            );
            assert!(lines >= 2, "{name}: the buttons didn't wrap");
            let last = items[items.len() - 1].0;
            let card = Rect::from_min_max(
                Pos2::new(place.left() + PAD, place.top() + ec::MARGIN),
                Pos2::new(
                    place.right() - PAD,
                    last.bottom() + ec::BODY_BOTTOM + ec::BORDER,
                ),
            );
            let frames: Vec<Rect> = rects(&h)
                .into_iter()
                .filter(|shape| shape.fill == ec::BG)
                .map(|shape| shape.rect)
                .collect();
            let [frame] = frames.as_slice() else {
                panic!("{name}'s cards: {frames:?}")
            };
            // Its right, but where an Edit wider than the body widens it (not yet clipped,
            // as `.err-card`'s `overflow: hidden` would).
            let sides = if items[0].0.right() > body_right {
                near(frame.left(), card.left())
                    && near(frame.top(), card.top())
                    && near(frame.bottom(), card.bottom())
            } else {
                near_rect(*frame, card)
            };
            assert!(sides, "{name}'s card: {frame:?}, not {card:?}");
        }
    }

    #[test]
    fn a_button_that_just_fits_after_another_stays_on_its_line() {
        // Edit Hole, the gap, and Show in timeline exactly as wide as the body: Show stays
        // on Edit's line, Copy as JSON on the next.
        let said = || {
            Said::of(
                &silico_core::Error::new(silico_core::ErrorCode::InvalidInput, "No tool")
                    .with_feature("c0000001"),
            )
        };
        let offer = Offer {
            edit: Some(false),
            show: true,
        };
        let labels = ["Edit Hole", "Show in timeline", "Copy as JSON"];
        let wide = card_in(
            Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(600.0, 300.0)),
            said(),
            Some("Hole"),
            offer,
        );
        let measured = buttons(&wide, &labels);
        let both = measured[0].0.width() + ec::ACTIONS_GAP + measured[1].0.width();
        let width = both + 2.0 * (PAD + ec::BORDER) + ec::BODY_LEFT + ec::SIDE;
        let place = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::new(width, 300.0));
        let h = card_in(place, said(), Some("Hole"), offer);
        let (body_left, body_right, _) = ec_body(place, LH);
        assert_eq!(body_right - body_left, both, "the body, the two exactly");
        let items = buttons(&h, &labels);
        assert!(near(items[1].1, items[0].1), "Show wrapped: {items:?}");
        assert_eq!(
            flowed(
                &items,
                body_left,
                body_right,
                ec::ACTIONS_GAP,
                BTN_H + ec::ACTIONS_GAP,
                "the buttons"
            ),
            2
        );
    }

    #[test]
    fn a_chip_keeps_a_name_of_34_characters_whole_and_cuts_a_longer_one_to_32_by_characters() {
        // 34 characters: whole.
        let whole = "k7f2qx3m.box.Edge12345678901234567";
        assert_eq!(whole.chars().count(), 34);
        assert_eq!(chip_text(whole), "k7f2qx3m.box.Edge12345678901234567");
        // 35: its first 32, then an ellipsis.
        let long = "k7f2qx3m.box.Edge123456789012345678";
        assert_eq!(long.chars().count(), 35);
        assert_eq!(chip_text(long), "k7f2qx3m.box.Edge123456789012345…");
        // 35 characters of two bytes each (70 bytes): cut at the 32nd character, not the
        // 32nd byte, and a name of 34 such characters (68 bytes) kept whole.
        let wide = "é".repeat(35);
        assert_eq!(chip_text(&wide), format!("{}…", "é".repeat(32)));
        assert_eq!(chip_text(&"é".repeat(34)), "é".repeat(34));
    }
}
