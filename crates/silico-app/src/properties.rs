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

use crate::icons::{Icon, IconSize};
use crate::modal::{self, Kind};
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

/// An error card (`.err-card`), a warning's if `warning`: its icon and `message`, and
/// `suggestion` after an arrow, if it has one.
///
/// SEAM: the rest of the prototype's card (DESIGN.md, *Agents as equal clients*; app.js's
/// `errCard`): its code chip (`.ec-code`), its Feature and Parameter rows (`.ec-kv`; a
/// dialog's card has its cause and Elements as lines, [`card_with`]), Copy as JSON, and Edit
/// and Show in timeline. The tab keeps each problem's message, cause, elements, and
/// suggestion (`tab::Said`); with the rest of the structured error kept, each is a row or a
/// button here.
pub(crate) fn card(ui: &mut Ui, warning: bool, message: &str, suggestion: Option<&str>) {
    card_with(ui, warning, message, &[], suggestion);
}

/// [`card`], with `details` under its message, each a line of its own (`errCard`'s message
/// and its Elements), before its suggestion.
pub(crate) fn card_with(
    ui: &mut Ui,
    warning: bool,
    message: &str,
    details: &[String],
    suggestion: Option<&str>,
) {
    let (fill, border, head, icon) = if warning {
        (
            theme::WARNING_CARD_BG,
            theme::WARNING_CARD_BORDER,
            theme::WARNING_CARD_HEAD,
            Icon::Warn,
        )
    } else {
        (
            theme::ERROR_CARD_BG,
            theme::ERROR_CARD_BORDER,
            theme::ERROR_CARD_HEAD,
            Icon::Error,
        )
    };
    let words = |text: &str| {
        let mut job = LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                line_height: Some(theme::BODY_LINE_HEIGHT),
                ..format(theme::BODY_SIZE, theme::TEXT_STRONG, false)
            },
        );
        job.wrap.max_width = f32::INFINITY;
        job
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(theme::BORDER_WIDTH, border))
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
                        // `align-items: flex-start`: the icon at the top of the first line.
                        let side = IconSize::Small.points();
                        let (at, _) = ui.allocate_exact_size(
                            vec2(side, theme::BODY_LINE_HEIGHT),
                            Sense::hover(),
                        );
                        icon.image(IconSize::Small)
                            .tint(head)
                            .paint_at(ui, Rect::from_min_size(at.min, Vec2::splat(side)));
                        ui.add(Label::new(words(message)).wrap().selectable(false));
                    });
                });
            egui::Frame::new()
                .inner_margin(theme::ERROR_CARD_BODY_PADDING)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for detail in details {
                        ui.add(Label::new(words(detail)).wrap().selectable(false));
                        ui.add_space(theme::ERROR_CARD_FIX_MARGIN);
                    }
                    let Some(suggestion) = suggestion else {
                        return;
                    };
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing = vec2(theme::ERROR_CARD_FIX_GAP, 0.0);
                        ui.add(
                            Label::new(LayoutJob::single_section(
                                "→".to_owned(),
                                TextFormat {
                                    line_height: Some(theme::BODY_LINE_HEIGHT),
                                    ..format(theme::BODY_SIZE, theme::ERROR_CARD_ACCENT, false)
                                },
                            ))
                            .selectable(false),
                        );
                        ui.add(Label::new(words(suggestion)).wrap().selectable(false));
                    });
                    ui.add_space(theme::ERROR_CARD_FIX_MARGIN);
                });
        });
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
/// key is empty): `text`, wrapped, in [`theme::SELECTION_BOX_SAID`].
pub(crate) fn said(ui: &mut Ui, text: &str) {
    let job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat {
            line_height: Some(theme::BODY_LINE_HEIGHT),
            ..format(theme::HEADER_SIZE, theme::SELECTION_BOX_SAID, false)
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

    use super::*;
    use crate::modal::tests::{
        at, at_opacity, cap_border, colour, fills_at, font, icon_at, kbd_border, near, rects,
        text_shape, widget,
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
}
