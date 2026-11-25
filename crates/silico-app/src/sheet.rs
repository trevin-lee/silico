//! The shortcut sheet (docs/design/m5-ui/DESIGN.md, state 7 and *Keyboard*; its
//! `styles.css`'s `.sheet` rules and `js/app.js`'s `renderShortcuts`): a modal, opened by
//! `?` anywhere outside a text field or by the status bar's Shortcuts, listing every command
//! of the registry (`crate::shortcuts`) that has keys, in three columns of groups, narrowed
//! by a filter on each command's name, its description, or its keys. Esc, its ×, or a click
//! on its backdrop closes it.
//!
//! Every text is a label, and each row's keys a label too (`Ctrl+B`), so the accessibility
//! tree has the sheet as it's drawn.

use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Rect, Sense, Stroke, TextFormat, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use crate::bar;
use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::modal;
use crate::shortcuts::{self, Entry, GROUPS, Group, REGISTRY};
use crate::start;
use crate::theme::{self, FieldKind};

/// The sheet's title, which its head says and the accessibility tree names it by.
pub(crate) const TITLE: &str = "Keyboard shortcuts";

/// The filter's label in the accessibility tree (the prototype's `aria-label`).
pub(crate) const FILTER_LABEL: &str = "Filter shortcuts";

/// What the sheet says when the filter finds nothing.
pub(crate) const NOTHING_FOUND: &str = "No shortcut matches.";

/// The note under the columns (the prototype's, js/app.js's shortcut sheet): its strong
/// words, then the rest.
const NOTE: [&str; 2] = [
    "Esc does the nearest thing first:",
    " ends a drag, then a sketch tool's chain, then stops a dialog's preview, then cancels \
     the dialog, then clears the selection, then stops the rebuild.",
];

/// The sheet while it's open: what's typed in its filter.
#[derive(Debug, Default)]
pub(crate) struct Sheet {
    pub filter: String,
    /// Whether the filter has had the keyboard yet: it takes it as the sheet opens.
    focused: bool,
}

/// The sheet's modal, by its name: what it's drawn on.
pub(crate) fn id() -> egui::Id {
    egui::Id::new("shortcut-sheet")
}

/// The sheet over the window, inert if `behind` a question asked over it (a quit's, a
/// replace's): whether it's asked closed (its × or its backdrop).
pub(crate) fn show(ctx: &egui::Context, sheet: &mut Sheet, behind: bool) -> bool {
    let window = ctx.content_rect().width();
    let width = theme::SHEET_WIDTH.min(window - theme::SHEET_WINDOW_MARGIN);
    let (closed, backdrop) = modal::show_sized(ctx, id(), width, behind, |ui| {
        ui.scope_builder(UiBuilder::new().id_salt("shortcut-sheet"), |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::Dialog);
                node.set_label(TITLE);
            });
            let closed = head(ui, sheet);
            modal::body(ui, |ui| body(ui, &sheet.filter));
            closed
        })
        .inner
    });
    closed || backdrop
}

/// The head (`.m-head`): the help icon and the title at its left; the filter (220 by 24,
/// the search icon in it) and the × at its right, 8 apart, all centred on its 24 high
/// middle. Whether the × was clicked.
fn head(ui: &mut Ui, sheet: &mut Sheet) -> bool {
    let pad = theme::MODAL_HEAD_PADDING;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, pad.sum().y + theme::SHEET_FILTER_HEIGHT),
        Sense::hover(),
    );
    // Its parts placed in a child, so none moves the body's cursor back up.
    let ui = &mut ui.new_child(UiBuilder::new().max_rect(rect));
    let middle = rect.top() + pad.topf() + theme::SHEET_FILTER_HEIGHT / 2.0;
    let left = rect.left() + pad.leftf();
    let right = rect.right() - pad.rightf();
    // The ×, as an app bar's button.
    let close_width =
        (ICON_SIZE + 2.0 * theme::APP_BAR_BUTTON_PADDING).max(theme::APP_BAR_BUTTON_SIZE);
    let close = Rect::from_min_size(
        pos2(
            right - close_width,
            middle - theme::APP_BAR_BUTTON_SIZE / 2.0,
        ),
        vec2(close_width, theme::APP_BAR_BUTTON_SIZE),
    );
    let closed = ui
        .scope_builder(UiBuilder::new().max_rect(close), |ui| {
            bar::button(ui, Icon::Close, None, "Close", true, None)
        })
        .inner
        .clicked();
    // The filter, its icon inside its left edge, its text 6 after the icon.
    let field = Rect::from_min_size(
        pos2(
            close.left() - theme::MODAL_HEAD_GAP - theme::SHEET_FILTER_WIDTH,
            middle - theme::SHEET_FILTER_HEIGHT / 2.0,
        ),
        vec2(theme::SHEET_FILTER_WIDTH, theme::SHEET_FILTER_HEIGHT),
    );
    let edit = egui::TextEdit::singleline(&mut sheet.filter).hint_text("Filter");
    let edit = theme::field_with_rooms(
        ui,
        edit,
        "shortcut-filter",
        FieldKind::Modify,
        false,
        [ICON_SIZE, 0.0],
    );
    let typed = ui.put(field, edit);
    ui.ctx()
        .accesskit_node_builder(typed.id, |node| node.set_label(FILTER_LABEL));
    if !sheet.focused {
        typed.request_focus();
        sheet.focused = true;
    }
    Icon::Search
        .image(IconSize::Small)
        .tint(theme::MODAL_TITLE)
        .paint_at(
            ui,
            Rect::from_min_size(
                pos2(field.left() + theme::BORDER_WIDTH, middle - ICON_SIZE / 2.0),
                Vec2::splat(ICON_SIZE),
            ),
        );
    // The help icon and the title, in what's left at the left.
    Icon::Help
        .image(IconSize::Small)
        .tint(theme::MODAL_TITLE)
        .paint_at(
            ui,
            Rect::from_min_size(pos2(left, middle - ICON_SIZE / 2.0), Vec2::splat(ICON_SIZE)),
        );
    let at = left + ICON_SIZE + theme::MODAL_HEAD_GAP;
    let mut job = LayoutJob::single_section(
        TITLE.to_owned(),
        modal::format(theme::MODAL_TITLE_SIZE, theme::MODAL_TITLE),
    );
    job.wrap =
        TextWrapping::truncate_at_width((field.left() - theme::MODAL_HEAD_GAP - at).max(1.0));
    let galley = ui.painter().layout_job(job);
    modal::label_at(ui, pos2(at, middle - galley.size().y / 2.0), galley);
    closed
}

/// The groups the filter `query` finds commands in, in the sheet's order, each with those
/// commands, in the registry's order: only commands with keys.
pub(crate) fn found(ctx: &egui::Context, query: &str) -> Vec<(Group, Vec<&'static Entry>)> {
    GROUPS
        .iter()
        .filter_map(|group| {
            let entries: Vec<&'static Entry> = REGISTRY
                .iter()
                .filter(|entry| {
                    entry.group == *group
                        && !entry.keys.is_empty()
                        && shortcuts::finds(ctx, entry, query)
                })
                .collect();
            (!entries.is_empty()).then_some((*group, entries))
        })
        .collect()
}

/// How many columns fit `width` (`columns: 3 260px`): three, or fewer where three of the
/// least width and their gaps don't fit, one at the least.
pub(crate) fn column_count(width: f32) -> usize {
    let fit = (width + theme::SHEET_COLUMN_GAP)
        / (theme::SHEET_COLUMN_MIN_WIDTH + theme::SHEET_COLUMN_GAP);
    (fit.floor().max(1.0) as usize).min(theme::SHEET_COLUMNS)
}

/// A group's height, with `rows` commands: its title, its padding, underline, and margin,
/// and its rows.
fn group_height(rows: usize) -> f32 {
    theme::BODY_LINE_HEIGHT
        + theme::SHEET_GROUP_TITLE_PADDING
        + theme::BORDER_WIDTH
        + theme::SHEET_GROUP_TITLE_MARGIN
        + rows as f32 * theme::SHEET_ROW_HEIGHT
}

/// The column of each of `heights`, in order, as CSS's balanced columns fill them: the
/// least height for which, filled in order, a column taking each group until the next
/// wouldn't fit (`break-inside: avoid`), they take no more than `count` columns; groups in
/// a column [`theme::SHEET_GROUP_GAP`] apart.
pub(crate) fn balanced(heights: &[f32], count: usize) -> Vec<usize> {
    let fill = |most: f32| -> Vec<usize> {
        let mut columns = Vec::with_capacity(heights.len());
        let (mut column, mut used) = (0, None::<f32>);
        for height in heights {
            let next = used.map_or(*height, |used| used + theme::SHEET_GROUP_GAP + height);
            if used.is_some() && next > most {
                column += 1;
                used = Some(*height);
            } else {
                used = Some(next);
            }
            columns.push(column);
        }
        columns
    };
    // Each run of groups' height is a column's that may be the tallest: the least for
    // which they fit.
    let mut tallest: Vec<f32> = Vec::new();
    for start in 0..heights.len() {
        let mut run = None::<f32>;
        for height in &heights[start..] {
            let next = run.map_or(*height, |run| run + theme::SHEET_GROUP_GAP + height);
            tallest.push(next);
            run = Some(next);
        }
    }
    tallest.sort_by(f32::total_cmp);
    tallest
        .into_iter()
        .map(fill)
        .find(|columns| columns.last().is_none_or(|last| *last < count.max(1)))
        .unwrap_or_else(|| (0..heights.len()).collect())
}

/// The body (`.m-body`): the columns of groups, or that nothing matches; then the note.
fn body(ui: &mut Ui, query: &str) {
    let width = ui.available_width();
    let found = found(ui.ctx(), query);
    if found.is_empty() {
        modal::text(ui, NOTHING_FOUND, theme::SHEET_NOTE_TEXT);
    } else {
        let count = column_count(width);
        let column_width = (width - (count - 1) as f32 * theme::SHEET_COLUMN_GAP) / count as f32;
        let heights: Vec<f32> = found
            .iter()
            .map(|(_, entries)| group_height(entries.len()))
            .collect();
        let columns = balanced(&heights, count);
        let top = ui.cursor().top();
        let mut bottoms = vec![top; count];
        let mut first = vec![true; count];
        for (((group, entries), height), column) in found.iter().zip(&heights).zip(&columns) {
            let (Some(bottom), Some(first)) = (bottoms.get_mut(*column), first.get_mut(*column))
            else {
                continue;
            };
            let y = if *first {
                *bottom
            } else {
                *bottom + theme::SHEET_GROUP_GAP
            };
            *first = false;
            let x =
                ui.max_rect().left() + *column as f32 * (column_width + theme::SHEET_COLUMN_GAP);
            draw_group(
                ui,
                Rect::from_min_size(pos2(x, y), vec2(column_width, *height)),
                *group,
                entries,
                query,
            );
            *bottom = y + height;
        }
        let tallest = bottoms.into_iter().fold(top, f32::max);
        ui.allocate_exact_size(vec2(width, tallest - top), Sense::hover());
    }
    note(ui, width);
}

/// A group (`.grp`) in `rect`: its title (`.grp-t`), underlined, then its rows.
fn draw_group(ui: &mut Ui, rect: Rect, group: Group, entries: &[&'static Entry], query: &str) {
    let title = |text: String, spaced: bool| {
        LayoutJob::single_section(
            text,
            TextFormat {
                extra_letter_spacing: if spaced {
                    theme::CAPITALS_LETTER_SPACING
                } else {
                    0.0
                },
                ..modal::format(theme::SHEET_GROUP_TITLE_SIZE, theme::SHEET_GROUP_TITLE_TEXT)
            },
        )
    };
    let name = ui
        .painter()
        .layout_job(title(group.name().to_uppercase(), true));
    modal::label_at(ui, rect.min, name);
    if let Some(note) = group.note() {
        let note = ui.painter().layout_job(title(note.to_owned(), false));
        modal::label_at(ui, pos2(rect.right() - note.size().x, rect.top()), note);
    }
    let line = rect.top() + theme::BODY_LINE_HEIGHT + theme::SHEET_GROUP_TITLE_PADDING;
    ui.painter().hline(
        rect.x_range(),
        line + theme::BORDER_WIDTH / 2.0,
        Stroke::new(theme::BORDER_WIDTH, theme::SHEET_GROUP_TITLE_BORDER),
    );
    let mut y = line + theme::BORDER_WIDTH + theme::SHEET_GROUP_TITLE_MARGIN;
    for entry in entries {
        row(
            ui,
            Rect::from_min_size(
                pos2(rect.left(), y),
                vec2(rect.width(), theme::SHEET_ROW_HEIGHT),
            ),
            entry,
            query,
        );
        y += theme::SHEET_ROW_HEIGHT;
    }
}

/// A command's row (`.sc-row`) in `rect`: its name at its left, what the filter found in
/// it marked; its keys' caps at its right, 3 apart.
fn row(ui: &mut Ui, rect: Rect, entry: &Entry, query: &str) {
    let caps = entry.command.caps(ui.ctx());
    let caps_width = start::key_caps_width_spaced(ui, &caps, theme::SHEET_KEY_CAP_GAP);
    let middle = rect.center().y;
    let mut job = marked(entry.label, query);
    job.wrap = TextWrapping::truncate_at_width(
        (rect.width() - caps_width - theme::SHEET_ROW_GAP).max(1.0),
    );
    let galley = ui.painter().layout_job(job);
    modal::label_at(
        ui,
        pos2(rect.left(), middle - galley.size().y / 2.0),
        galley,
    );
    start::key_caps_spaced(
        ui,
        pos2(rect.right(), middle),
        &caps,
        false,
        theme::SHEET_KEY_CAP_GAP,
    );
    let keys = Rect::from_min_max(
        pos2(
            rect.right() - caps_width,
            middle - theme::KEY_CAP_HEIGHT / 2.0,
        ),
        pos2(rect.right(), middle + theme::KEY_CAP_HEIGHT / 2.0),
    );
    let said = shortcuts::joined(ui.ctx(), &caps);
    ui.interact(
        keys,
        ui.id().with(("sheet-keys", entry.command)),
        Sense::hover(),
    )
    .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &said));
}

/// `label` in the row's type, each place `query` is found in it, its case aside, marked
/// (`mark`): white on the selection's blue at 35%.
pub(crate) fn marked(label: &str, query: &str) -> LayoutJob {
    let plain = modal::format(theme::BODY_SIZE, theme::SHEET_ROW_TEXT);
    let found = TextFormat {
        color: theme::SHEET_MATCH_TEXT,
        background: theme::SHEET_MATCH_BG,
        ..plain.clone()
    };
    let lower = |c: char| c.to_lowercase().collect::<String>();
    let query: Vec<String> = query.trim().chars().map(lower).collect();
    let chars: Vec<(usize, char)> = label.char_indices().collect();
    let mut job = LayoutJob::default();
    let mut from = 0;
    let mut at = 0;
    while !query.is_empty() && at + query.len() <= chars.len() {
        let here = chars[at..at + query.len()]
            .iter()
            .zip(&query)
            .all(|((_, c), q)| lower(*c) == *q);
        if !here {
            at += 1;
            continue;
        }
        let start = chars[at].0;
        let end = chars
            .get(at + query.len())
            .map_or(label.len(), |(at, _)| *at);
        if from < start {
            job.append(&label[from..start], 0.0, plain.clone());
        }
        job.append(&label[start..end], 0.0, found.clone());
        from = end;
        at += query.len();
    }
    if from < label.len() {
        job.append(&label[from..], 0.0, plain);
    }
    job
}

/// The note under the columns (`.note`): 6 under them, a rule, then 8 to its words, Esc's
/// order, its first words strong.
fn note(ui: &mut Ui, width: f32) {
    ui.add_space(theme::SHEET_NOTE_MARGIN);
    let (rule, _) = ui.allocate_exact_size(vec2(width, theme::BORDER_WIDTH), Sense::hover());
    ui.painter().hline(
        rule.x_range(),
        rule.center().y,
        Stroke::new(theme::BORDER_WIDTH, theme::SHEET_NOTE_BORDER),
    );
    ui.add_space(theme::SHEET_NOTE_PADDING);
    let mut job = LayoutJob::default();
    job.append(
        NOTE[0],
        0.0,
        modal::format(theme::HEADER_SIZE, theme::SHEET_NOTE_STRONG),
    );
    job.append(
        NOTE[1],
        0.0,
        modal::format(theme::HEADER_SIZE, theme::SHEET_NOTE_TEXT),
    );
    job.wrap.max_width = width;
    let galley = ui.painter().layout_job(job);
    ui.add(egui::Label::new(galley).selectable(false));
}

#[cfg(test)]
mod tests {
    use eframe::egui::epaint::{RectShape, TextShape};
    use eframe::egui::{CornerRadius, Pos2};
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::modal::tests::{
        at, colour, font, icon_at, near, painted, rects, rgba, sized, text_shape, widget,
    };
    use eframe::egui::Color32;

    /// The prototype's geometry (`styles.css`: `.modal`, `.m-head`, `.m-body`, `.sheet`,
    /// `.cols`, `.grp`, `.grp-t`, `.sc-row`, `.kbd`, `.note`, `mark`; app.js's
    /// `renderShortcuts`, its field `width:220px;height:24px`), typed from it, never from
    /// theme.rs.
    mod css {
        use eframe::egui::Color32;

        pub const G250: Color32 = Color32::from_rgb(0x27, 0x26, 0x26);
        pub const G350: Color32 = Color32::from_rgb(0x52, 0x51, 0x51);
        pub const G550: Color32 = Color32::from_rgb(0x93, 0x90, 0x90);
        pub const G750: Color32 = Color32::from_rgb(0xcf, 0xcf, 0xcf);
        pub const G800: Color32 = Color32::from_rgb(0xe0, 0xdf, 0xdf);
        pub const G1000: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
        /// `.sheet mark`: `rgba(24,106,221,.35)`, `#fff`.
        pub const MARK: [u8; 4] = [24, 106, 221, 89];

        /// `body`'s 12/16; `--fs-small`; `--icon`.
        pub const FS: f32 = 12.0;
        pub const LH: f32 = 16.0;
        pub const FS_SMALL: f32 = 11.0;
        pub const ICON: f32 = 14.0;
        /// `.sheet { width: 920px }`, `border-box`, its 1 px border.
        pub const SHEET_W: f32 = 920.0;
        pub const BORDER: f32 = 1.0;
        /// `.m-head`: `gap: 8px; padding: 12px 12px 8px 16px`, its title 14.
        pub const HEAD_TOP: f32 = 12.0;
        pub const HEAD_RIGHT: f32 = 12.0;
        pub const HEAD_BOTTOM: f32 = 8.0;
        pub const HEAD_LEFT: f32 = 16.0;
        pub const HEAD_GAP: f32 = 8.0;
        pub const HEAD_FS: f32 = 14.0;
        /// The filter: `width:220px;height:24px`; `.field input { padding: 0 6px }`.
        pub const FILTER_W: f32 = 220.0;
        pub const FILTER_H: f32 = 24.0;
        pub const INPUT_PAD: f32 = 6.0;
        /// `.ab-btn`: 22 high, `padding: 0 5px` about a 14 icon.
        pub const AB_H: f32 = 22.0;
        pub const AB_W: f32 = 24.0;
        /// `.m-body`: `padding: 0 16px 12px`.
        pub const BODY_SIDE: f32 = 16.0;
        /// `.cols { columns: 3 260px; column-gap: 28px }`; `.grp { margin-bottom: 14px }`.
        pub const COLUMN_GAP: f32 = 28.0;
        pub const GROUP_GAP: f32 = 14.0;
        /// `.grp-t`: 10.5, `letter-spacing: .5px`, `padding-bottom: 4px; border-bottom: 1px
        /// solid var(--g250); margin-bottom: 4px`.
        pub const TITLE_FS: f32 = 10.5;
        pub const TITLE_SPACING: f32 = 0.5;
        pub const TITLE_PAD: f32 = 4.0;
        pub const TITLE_MARGIN: f32 = 4.0;
        /// `.sc-row { height: 22px }`; `.sheet .sc-row .keys { gap: 3px }`; `.kbd`: 16 high,
        /// 16 wide at the least, `border-radius: 3px`, 10.5.
        pub const ROW_H: f32 = 22.0;
        pub const KEYS_GAP: f32 = 3.0;
        pub const KBD_H: f32 = 16.0;
        pub const KBD_MIN_W: f32 = 16.0;
        pub const KBD_R: u8 = 3;
        pub const KBD_FS: f32 = 10.5;
        /// `.note`: `margin-top:6px; border-top:1px solid var(--g250); padding-top:8px`.
        pub const NOTE_TOP: f32 = 6.0;
        pub const NOTE_PAD: f32 = 8.0;
        /// `.modal { max-width: calc(100vw - 40px) }`.
        pub const MODAL_MARGIN: f32 = 40.0;
        /// `.cols { columns: 3 260px }`: a column's least width.
        pub const COLUMN_MIN_W: f32 = 260.0;
        /// `.sheet .sc-row { gap: 8px }`: between a row's name and its keys.
        pub const ROW_GAP: f32 = 8.0;
    }

    /// The text of `range` of `text`.
    fn part<'t>(text: &'t str, range: &std::ops::Range<egui::text::ByteIndex>) -> &'t str {
        &text[range.start.0..range.end.0]
    }

    /// The window: wider than the sheet and its margins.
    const WINDOW: Vec2 = Vec2::new(1280.0, 800.0);

    fn sheet(filter: &str) -> Harness<'static, (Sheet, bool)> {
        sheet_in(filter, WINDOW)
    }

    /// [`sheet`] in a window of `window`.
    fn sheet_in(filter: &str, window: Vec2) -> Harness<'static, (Sheet, bool)> {
        let open = Sheet {
            filter: filter.to_owned(),
            focused: false,
        };
        let mut h = Harness::builder().with_size(window).build_ui_state(
            |ui, (sheet, closed): &mut (Sheet, bool)| {
                *closed |= show(ui.ctx(), sheet, false);
            },
            (open, false),
        );
        theme::apply(&h.ctx);
        h.run_steps(4);
        h
    }

    /// The sheet's frame: its fill's shape, Gray.200 outlined in Gray.250.
    fn frame_of<S>(h: &Harness<'_, S>) -> Rect {
        let mut found: Vec<RectShape> = rects(h)
            .into_iter()
            .filter(|shape| {
                shape.fill == Color32::from_rgb(0x21, 0x21, 0x21) && shape.stroke.color == css::G250
            })
            .collect();
        assert_eq!(found.len(), 1, "the sheet's frames");
        found.remove(0).rect
    }

    /// The texts painted as `text`, in the order painted.
    fn texts<S>(h: &Harness<'_, S>, text: &str) -> Vec<TextShape> {
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(shown) if shown.galley.text() == text => Some(shown),
                _ => None,
            })
            .collect()
    }

    /// The key caps painted with their tops at `top`, left to right.
    fn caps_at<S>(h: &Harness<'_, S>, top: f32, within: Rect) -> Vec<RectShape> {
        let mut found: Vec<RectShape> = rects(h)
            .into_iter()
            .filter(|shape| {
                near(shape.rect.top(), top)
                    && near(shape.rect.height(), css::KBD_H)
                    && within.contains_rect(shape.rect)
                    && shape.corner_radius == CornerRadius::same(css::KBD_R)
            })
            .collect();
        found.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
        found
    }

    #[test]
    fn the_sheet_is_920_wide_and_its_head_is_the_prototypes() {
        let h = sheet("");
        let frame = frame_of(&h);
        assert!(near(frame.width(), css::SHEET_W), "{frame:?}");
        // Centred: across, exactly (920 and 1280 are both even); down, its top where egui
        // puts a window, half the room left over it, on a whole point (its height is odd).
        let off = frame.center() - (WINDOW / 2.0).to_pos2();
        assert_eq!(off.x, 0.0, "{frame:?}");
        assert_eq!(
            frame.min.y,
            (WINDOW.y / 2.0 - frame.height() / 2.0).round(),
            "{frame:?}"
        );
        let inside = frame.shrink(css::BORDER);
        // The head's middle: 12 under its top, half the filter's 24 down.
        let middle = inside.top() + css::HEAD_TOP + css::FILTER_H / 2.0;
        // At its left the help icon, then 8, the title, 14 in Gray.1000.
        let help = Rect::from_min_size(
            Pos2::new(inside.left() + css::HEAD_LEFT, middle - css::ICON / 2.0),
            Vec2::splat(css::ICON),
        );
        assert_eq!(icon_at(&h, help), Some(css::G1000), "the help icon");
        // The head's title: the first painted (the sheet lists itself, under Help, after).
        let title = texts(&h, TITLE).remove(0);
        font(&title, css::HEAD_FS, "the title");
        assert_eq!(colour(&title), css::G1000);
        at(
            title.pos,
            Pos2::new(
                help.right() + css::HEAD_GAP,
                middle - title.galley.size().y / 2.0,
            ),
            "the title",
        );
        // At its right the ×, 12 in, 24 by 22; 8 before it, the filter, 220 by 24, its
        // search icon inside its left edge and its text 6 after that.
        let close = widget(&h, Role::Button, "Close");
        assert!(
            near(close.right(), inside.right() - css::HEAD_RIGHT),
            "{close:?}"
        );
        assert!(
            near(close.width(), css::AB_W) && near(close.height(), css::AB_H),
            "{close:?}"
        );
        assert!(near(close.center().y, middle));
        let filter = widget(&h, Role::TextInput, FILTER_LABEL);
        assert!(near(filter.width(), css::FILTER_W), "{filter:?}");
        assert!(near(filter.height(), css::FILTER_H), "{filter:?}");
        assert!(near(filter.right(), close.left() - css::HEAD_GAP));
        assert!(near(filter.center().y, middle));
        let search = Rect::from_min_size(
            Pos2::new(filter.left() + css::BORDER, middle - css::ICON / 2.0),
            Vec2::splat(css::ICON),
        );
        assert!(icon_at(&h, search).is_some(), "the search icon");
        let hint = text_shape(&h, "Filter");
        assert!(
            near(hint.pos.x, search.right() + css::INPUT_PAD),
            "the filter's words at {:?}, not {}",
            hint.pos,
            search.right() + css::INPUT_PAD
        );
        // The head ends 8 under the filter.
        let groups = texts(&h, "FILE");
        assert_eq!(groups.len(), 1);
        assert!(near(
            groups[0].pos.y,
            inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM
        ));
    }

    #[test]
    fn the_groups_fill_three_balanced_columns_28_apart_each_group_14_under_the_last() {
        let h = sheet("");
        let inside = frame_of(&h).shrink(css::BORDER);
        let body = inside.shrink2(Vec2::new(css::BODY_SIDE, 0.0));
        let column = (body.width() - 2.0 * css::COLUMN_GAP) / 3.0;
        let top = inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM;
        // Each group's height: its title's 16 line, 4, its 1 underline, 4, and 22 a row.
        let height =
            |rows: f32| css::LH + css::TITLE_PAD + 1.0 + css::TITLE_MARGIN + rows * css::ROW_H;
        // By hand: File 5 rows (135), Edit 5 (135: Undo, Redo, Search commands, Rename,
        // Delete), Rebuild 4 (113: Rebuild, Rebuild everything, Reload, Stop), View 13 (311,
        // Look At its last key's), Selection 5 (135), Dialogs 2 (69), Sketch 10 (245, Sketch
        // without relations among them), Help 1 (47). The least height that takes three
        // columns, filled in order, is 460: below it View can't take Selection (311 + 14 +
        // 135 = 460), and Selection to Help (135 + 14 + 69 + 14 + 245 + 14 + 47 = 538) take
        // two more. At 460, File, Edit, and Rebuild (135 + 14 + 135 + 14 + 113 = 411); View
        // and Selection (460); Dialogs, Sketch, and Help (69 + 14 + 245 + 14 + 47 = 389).
        let placed = [
            ("FILE", 0, 0.0),
            ("EDIT", 0, height(5.0) + css::GROUP_GAP),
            (
                "REBUILD",
                0,
                height(5.0) + height(5.0) + 2.0 * css::GROUP_GAP,
            ),
            ("VIEW", 1, 0.0),
            ("SELECTION", 1, height(13.0) + css::GROUP_GAP),
            ("DIALOGS", 2, 0.0),
            ("SKETCH", 2, height(2.0) + css::GROUP_GAP),
            ("HELP", 2, height(2.0) + height(10.0) + 2.0 * css::GROUP_GAP),
        ];
        for (name, at_column, down) in placed {
            let shown = texts(&h, name);
            assert_eq!(shown.len(), 1, "{name}");
            let want = Pos2::new(
                body.left() + at_column as f32 * (column + css::COLUMN_GAP),
                top + down,
            );
            at(shown[0].pos, want, name);
            // `.grp-t`: 10.5 capitals, half a point apart, Gray.550.
            font(&shown[0], css::TITLE_FS, name);
            assert_eq!(colour(&shown[0]), css::G550, "{name}");
            assert_eq!(
                shown[0].galley.job.sections[0].format.extra_letter_spacing,
                css::TITLE_SPACING
            );
        }
        // View's note at its title's right, as it's written (no capitals, no spacing).
        let note = text_shape(&h, "Fusion’s mouse");
        let view_right = body.left() + 2.0 * column + css::COLUMN_GAP;
        assert!(
            near(note.pos.x + note.galley.size().x, view_right),
            "{:?}",
            note.pos
        );
        assert!(near(note.pos.y, top));
        assert_eq!(note.galley.job.sections[0].format.extra_letter_spacing, 0.0);
        // Under File's title, 4 down, its Gray.250 underline across the column.
        let line = top + css::LH + css::TITLE_PAD + 0.5;
        let underlined = painted(&h).into_iter().any(|shape| match shape {
            egui::Shape::LineSegment { points, stroke } => {
                near(points[0].y, line)
                    && near(points[0].x, body.left())
                    && near(points[1].x, body.left() + column)
                    && stroke.color == css::G250
                    && stroke.width == 1.0
            }
            _ => false,
        });
        assert!(underlined, "File's underline");
    }

    #[test]
    fn a_row_is_its_name_and_its_caps_3_apart_at_the_columns_right() {
        let h = sheet("");
        let inside = frame_of(&h).shrink(css::BORDER);
        let body = inside.shrink2(Vec2::new(css::BODY_SIDE, 0.0));
        let column = (body.width() - 2.0 * css::COLUMN_GAP) / 3.0;
        let top = inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM;
        // File's first row, Save As… its fourth: 4 + 1 + 4 under the title's line.
        let rows = top + css::LH + css::TITLE_PAD + 1.0 + css::TITLE_MARGIN;
        for (name, nth, caps) in [
            ("New part", 0.0, &["Ctrl", "N"][..]),
            ("Save As…", 3.0, &["Ctrl", "Shift", "S"][..]),
        ] {
            let row = Rect::from_min_size(
                Pos2::new(body.left(), rows + nth * css::ROW_H),
                Vec2::new(column, css::ROW_H),
            );
            let words = text_shape(&h, name);
            font(&words, css::FS, name);
            assert_eq!(colour(&words), css::G750, "{name}");
            at(
                words.pos,
                Pos2::new(row.left(), row.center().y - words.galley.size().y / 2.0),
                name,
            );
            let drawn = caps_at(&h, row.center().y - css::KBD_H / 2.0, row);
            assert_eq!(drawn.len(), caps.len(), "{name}'s caps");
            assert!(near(drawn[drawn.len() - 1].rect.right(), row.right()));
            for pair in drawn.windows(2) {
                assert!(near(
                    pair[1].rect.left() - pair[0].rect.right(),
                    css::KEYS_GAP
                ));
            }
            for cap in &drawn {
                assert!(cap.rect.width() >= css::KBD_MIN_W);
                assert_eq!(cap.fill, css::G250);
                assert!(cap.stroke.is_empty(), "{name}'s cap: {:?}", cap.stroke);
                modal::tests::kbd_border(
                    &modal::tests::cap_border(&h, cap.rect),
                    cap.rect,
                    css::G350,
                    name,
                );
            }
            for cap in caps.iter() {
                let shown = texts(&h, cap)
                    .into_iter()
                    .find(|shown| row.contains(shown.pos))
                    .unwrap_or_else(|| panic!("{name}'s {cap}"));
                font(&shown, css::KBD_FS, cap);
                assert_eq!(colour(&shown), css::G800);
            }
            // Its keys are one label, as the platform joins them.
            assert!(h.query_by_label(&caps.join("+")).is_some(), "{name}'s keys");
        }
    }

    #[test]
    fn the_filter_keeps_the_rows_it_finds_and_marks_it_in_their_names() {
        let h = sheet("save");
        // Save and Save As… alone, under File; the rest gone.
        assert_eq!(texts(&h, "FILE").len(), 1);
        for gone in ["EDIT", "REBUILD", "VIEW", "SELECTION", "DIALOGS", "HELP"] {
            assert!(texts(&h, gone).is_empty(), "{gone}");
        }
        assert!(h.query_by_label("New part").is_none());
        assert!(h.query_by_label("Save As…").is_some());
        // "Save" marked in each: white on Blue.500 at 35%, the rest as it was.
        let shown: Vec<TextShape> = painted(&h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(shown) if shown.galley.text().starts_with("Save") => Some(shown),
                _ => None,
            })
            .collect();
        assert_eq!(shown.len(), 2);
        for text in &shown {
            let first = &text.galley.job.sections[0];
            assert_eq!(part(text.galley.text(), &first.byte_range), "Save");
            assert_eq!(first.format.background, rgba(css::MARK));
            assert_eq!(first.format.color, css::G1000);
            if let Some(rest) = text.galley.job.sections.get(1) {
                assert_eq!(rest.format.background, Color32::TRANSPARENT);
                assert_eq!(rest.format.color, css::G750);
            }
        }
        // By key: F6 finds Fit alone, under View.
        let h = sheet("F6");
        assert_eq!(texts(&h, "VIEW").len(), 1);
        assert!(h.query_by_label("Fit").is_some());
        assert!(h.query_by_label("Home view").is_none());
        assert!(texts(&h, "FILE").is_empty());
        // Nothing found: said.
        let h = sheet("no such command");
        let said = text_shape(&h, NOTHING_FOUND);
        font(&said, css::FS, "the note");
        assert_eq!(colour(&said), css::G550);
    }

    #[test]
    fn the_note_is_6_under_the_tallest_column_over_its_rule_and_8_above_its_words() {
        let h = sheet("");
        let inside = frame_of(&h).shrink(css::BORDER);
        let top = inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM;
        // The tallest column: View's and Selection's, their 18 rows and a gap (460, by hand;
        // File's, Edit's, and Rebuild's are 411, and Dialogs', Sketch's, and Help's 389).
        let title = css::LH + css::TITLE_PAD + 1.0 + css::TITLE_MARGIN;
        let tallest = top + 2.0 * title + 18.0 * css::ROW_H + css::GROUP_GAP;
        assert!(near(tallest - top, 460.0));
        let rule = tallest + css::NOTE_TOP;
        let ruled = painted(&h).into_iter().any(|shape| match shape {
            egui::Shape::LineSegment { points, stroke } => {
                near(points[0].y, rule + 0.5) && stroke.color == css::G250
            }
            _ => false,
        });
        assert!(ruled, "the note's rule at {rule}");
        let note = painted(&h)
            .into_iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(shown) if shown.galley.text().starts_with(NOTE[0]) => Some(shown),
                _ => None,
            })
            .expect("the note");
        assert!(
            near(note.pos.y, rule + 1.0 + css::NOTE_PAD),
            "{:?}",
            note.pos
        );
        font(&note, css::FS_SMALL, "the note");
        // Its words, typed from js/app.js's `.note`.
        assert_eq!(
            note.galley.text(),
            "Esc does the nearest thing first: ends a drag, then a sketch tool's chain, then \
             stops a dialog's preview, then cancels the dialog, then clears the selection, then \
             stops the rebuild."
        );
        let sections = &note.galley.job.sections;
        assert_eq!(sections[0].format.color, css::G1000, "its strong words");
        assert_eq!(sections[1].format.color, css::G550, "the rest");
    }

    #[test]
    fn its_close_and_its_backdrop_close_it() {
        let mut h = sheet("");
        assert!(!h.state().1);
        h.get_by_role_and_label(Role::Button, "Close").click();
        h.run_steps(2);
        assert!(h.state().1);
        let mut h = sheet("");
        h.event(egui::Event::PointerButton {
            pos: Pos2::new(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.event(egui::Event::PointerButton {
            pos: Pos2::new(10.0, 10.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(2);
        assert!(h.state().1, "a click on the backdrop");
    }

    #[test]
    fn columns_balance_by_hand_and_narrow_windows_take_fewer() {
        // Three of 260 and two gaps of 28 need 836.
        assert_eq!(column_count(836.0), 3);
        assert_eq!(column_count(835.0), 2);
        assert_eq!(column_count(548.0), 2);
        assert_eq!(column_count(547.0), 1);
        assert_eq!(column_count(10.0), 1);
        // Four groups of 10 in two columns: 10 + 14 + 10 = 34 each.
        assert_eq!(balanced(&[10.0; 4], 2), [0, 0, 1, 1]);
        // A tall one alone: it sets the height, the rest share.
        assert_eq!(balanced(&[100.0, 10.0, 10.0], 2), [0, 1, 1]);
        assert_eq!(balanced(&[], 3), Vec::<usize>::new());
        assert_eq!(balanced(&[5.0], 3), [0]);
        // One column takes every group, however tall: two of 10 fill it to 34 exactly.
        assert_eq!(balanced(&[10.0, 10.0], 1), [0, 0]);
        assert_eq!(balanced(&[10.0, 20.0, 30.0], 1), [0, 0, 0]);
    }

    #[test]
    fn marking_is_case_blind_and_finds_every_place() {
        let job = marked("Hide or show", "O");
        let parts: Vec<(&str, bool)> = job
            .sections
            .iter()
            .map(|section| {
                (
                    part(&job.text, &section.byte_range),
                    section.format.background != Color32::TRANSPARENT,
                )
            })
            .collect();
        assert_eq!(
            parts,
            [
                ("Hide ", false),
                ("o", true),
                ("r sh", false),
                ("o", true),
                ("w", false)
            ]
        );
        // Past a character of more than a byte.
        let job = marked("Open…", "n…");
        assert_eq!(part(&job.text, &job.sections[1].byte_range), "n…");
        assert_eq!(marked("Fit", "").sections.len(), 1);
        // Found to its end: one section, marked, nothing after it (`<mark>Fit</mark>`).
        let job = marked("Fit", "fit");
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.background, rgba(css::MARK));
        let job = marked("Save As…", "as…");
        assert_eq!(
            job.sections
                .iter()
                .map(|section| part(&job.text, &section.byte_range))
                .collect::<Vec<_>>(),
            ["Save ", "As…"]
        );
        assert!(marked("", "").sections.is_empty());
    }

    /// A window narrower than the sheet and its margins, tall enough for one column of
    /// every group.
    const NARROW: Vec2 = Vec2::new(500.0, 1500.0);

    /// Each group's height, with `rows` commands, by hand: its title's 16 line, 4, its 1
    /// underline, 4, and 22 a row.
    fn group_tall(rows: f32) -> f32 {
        css::LH + css::TITLE_PAD + 1.0 + css::TITLE_MARGIN + rows * css::ROW_H
    }

    #[test]
    fn a_narrow_window_takes_the_sheet_40_narrower_and_every_group_in_one_column() {
        // 460 of a 500 window.
        let h = sheet_in("", NARROW);
        let frame = frame_of(&h);
        assert!(
            near(frame.width(), NARROW.x - css::MODAL_MARGIN),
            "{frame:?}"
        );
        at(
            frame.center(),
            Pos2::new(NARROW.x / 2.0, frame.center().y),
            "the sheet's middle",
        );
        let inside = frame.shrink(css::BORDER);
        let body = inside.shrink2(Vec2::new(css::BODY_SIDE, 0.0));
        // 426 between the body's sides, short of the 548 two columns of 260 and their gap
        // take: one column, every group in it in order, 14 apart.
        assert!(body.width() < 2.0 * css::COLUMN_MIN_W + css::COLUMN_GAP);
        let top = inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM;
        let mut down = 0.0;
        // Edit's five, as the prototype's `CMD` has them: Undo, Redo, Search commands (S),
        // Rename, and Delete.
        for (name, rows) in [
            ("FILE", 5.0),
            ("EDIT", 5.0),
            ("REBUILD", 4.0),
            ("VIEW", 13.0),
            ("SELECTION", 5.0),
            ("DIALOGS", 2.0),
            ("SKETCH", 10.0),
            ("HELP", 1.0),
        ] {
            let shown = texts(&h, name);
            assert_eq!(shown.len(), 1, "{name}");
            at(shown[0].pos, Pos2::new(body.left(), top + down), name);
            down += group_tall(rows) + css::GROUP_GAP;
        }
    }

    #[test]
    fn the_title_is_whole_where_it_fits_and_cut_short_8_before_the_filter_where_not() {
        let h = sheet("");
        let title = texts(&h, TITLE).remove(0);
        assert!(!title.galley.elided, "the title cut short in a wide window");
        // From 400 to 430 wide, a point at a time: the head's 16, the help icon's 14, 8, then
        // 8, the filter's 220, 8, the ×'s 24, and 12 leave the title 48 to 78 of the 130 or
        // so it needs. It's cut short, and ends no nearer the filter than the head's gap.
        for width in 400..=430 {
            let h = sheet_in("", Vec2::new(width as f32, 800.0));
            let title = texts(&h, TITLE).remove(0);
            let filter = widget(&h, Role::TextInput, FILTER_LABEL);
            let right = title.pos.x + title.galley.size().x;
            assert!(title.galley.elided, "{width} wide: the title whole");
            assert!(
                right <= filter.left() - css::HEAD_GAP + egui::emath::GUI_ROUNDING,
                "{width} wide: the title ends at {right}, the filter starts at {}",
                filter.left()
            );
        }
    }

    /// Each row's name painted (12, as `.sc-row`'s), and where the first of the caps on its
    /// line starts, in the order painted.
    fn row_names<S>(h: &Harness<'_, S>) -> Vec<(TextShape, f32)> {
        let caps: Vec<Rect> = rects(h)
            .into_iter()
            .filter(|shape| {
                near(shape.rect.height(), css::KBD_H)
                    && shape.corner_radius == CornerRadius::same(css::KBD_R)
            })
            .map(|shape| shape.rect)
            .collect();
        painted(h)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(shown)
                    if shown.galley.job.sections[0].format.font_id.size == css::FS
                        && REGISTRY.iter().any(|entry| {
                            !entry.keys.is_empty() && entry.label == shown.galley.text()
                        }) =>
                {
                    let middle = shown.pos.y + shown.galley.size().y / 2.0;
                    let first = caps
                        .iter()
                        .filter(|cap| near(cap.center().y, middle) && cap.left() > shown.pos.x)
                        .map(|cap| cap.left())
                        .fold(f32::INFINITY, f32::min);
                    Some((shown, first))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_rows_name_is_whole_where_it_fits_and_cut_short_8_before_its_caps_where_not() {
        let every = REGISTRY
            .iter()
            .filter(|entry| !entry.keys.is_empty())
            .count();
        let h = sheet("");
        let shown = row_names(&h);
        assert_eq!(shown.len(), every);
        for (name, _) in &shown {
            assert!(!name.galley.elided, "{:?} cut short", name.galley.text());
        }
        // 230 to 250 wide: the sheet 190 to 210, its one column 156 to 176 between the
        // body's sides. A name too long for what its caps leave is cut short, never nearer
        // them than the row's gap. 1500 high, so the one column shows every row.
        for width in [230.0, 240.0, 250.0] {
            let h = sheet_in("", Vec2::new(width, 1500.0));
            let shown = row_names(&h);
            assert_eq!(shown.len(), every, "{width} wide");
            assert!(
                shown.iter().any(|(name, _)| name.galley.elided),
                "{width} wide: no name cut short"
            );
            for (name, caps) in &shown {
                let right = name.pos.x + name.galley.size().x;
                assert!(
                    right <= caps - css::ROW_GAP + egui::emath::GUI_ROUNDING,
                    "{width} wide: {:?} ends at {right}, its caps start at {caps}",
                    name.galley.text()
                );
            }
        }
    }

    #[test]
    fn a_rows_keys_are_one_label_over_its_caps() {
        let h = sheet("");
        let inside = frame_of(&h).shrink(css::BORDER);
        let body = inside.shrink2(Vec2::new(css::BODY_SIDE, 0.0));
        let column = (body.width() - 2.0 * css::COLUMN_GAP) / 3.0;
        let top = inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM;
        let rows = top + css::LH + css::TITLE_PAD + 1.0 + css::TITLE_MARGIN;
        for (nth, said, count) in [(0.0, "Ctrl+N", 2), (3.0, "Ctrl+Shift+S", 3)] {
            let row = Rect::from_min_size(
                Pos2::new(body.left(), rows + nth * css::ROW_H),
                Vec2::new(column, css::ROW_H),
            );
            let drawn = caps_at(&h, row.center().y - css::KBD_H / 2.0, row);
            assert_eq!(drawn.len(), count, "{said}'s caps");
            // From its first cap's left to the column's right, the caps' 16 high, centred
            // on the row's 22.
            let keys = Rect::from_min_max(
                Pos2::new(drawn[0].rect.left(), row.center().y - css::KBD_H / 2.0),
                Pos2::new(row.right(), row.center().y + css::KBD_H / 2.0),
            );
            sized(h.get_by_label(said).rect(), keys, said);
        }
    }

    #[test]
    fn selections_and_sketchs_notes_are_at_their_titles_right_as_the_prototype_writes_them() {
        // The prototype's `notes`: Selection's "outside sketches", in the second column under
        // View, and Sketch's "while a sketch is open", in the third under Dialogs, each at
        // the right of its title's line, as written (no capitals, no spacing).
        let h = sheet("");
        let inside = frame_of(&h).shrink(css::BORDER);
        let body = inside.shrink2(Vec2::new(css::BODY_SIDE, 0.0));
        let column = (body.width() - 2.0 * css::COLUMN_GAP) / 3.0;
        let top = inside.top() + css::HEAD_TOP + css::FILTER_H + css::HEAD_BOTTOM;
        let height =
            |rows: f32| css::LH + css::TITLE_PAD + 1.0 + css::TITLE_MARGIN + rows * css::ROW_H;
        for (words, right, down) in [
            (
                "outside sketches",
                body.left() + 2.0 * column + css::COLUMN_GAP,
                height(13.0) + css::GROUP_GAP,
            ),
            (
                "while a sketch is open",
                body.right(),
                height(2.0) + css::GROUP_GAP,
            ),
        ] {
            let note = text_shape(&h, words);
            assert!(
                near(note.pos.x + note.galley.size().x, right),
                "{words}: {:?}",
                note.pos
            );
            assert!(near(note.pos.y, top + down), "{words}: {:?}", note.pos);
            font(&note, css::TITLE_FS, words);
            assert_eq!(colour(&note), css::G550);
            assert_eq!(note.galley.job.sections[0].format.extra_letter_spacing, 0.0);
        }
    }

    #[test]
    fn x_is_listed_under_selection_as_faces_and_under_sketch_as_construction() {
        // One key in two groups that never apply at once (the registry's one exception):
        // the sheet lists each row with its own X, Faces in Selection's column (the second,
        // under View) and Construction in Sketch's (the third, under Dialogs, above Help).
        let h = sheet("");
        let one = |text: &str| {
            let mut found = texts(&h, text);
            assert_eq!(found.len(), 1, "{text}");
            found.remove(0)
        };
        let middle = |shown: &TextShape| shown.pos.y + shown.galley.size().y / 2.0;
        let (selection, sketch, help) = (one("SELECTION"), one("SKETCH"), one("HELP"));
        let (faces, construction) = (one("Faces"), one("Construction"));
        assert!(near(faces.pos.x, selection.pos.x), "{:?}", faces.pos);
        assert!(faces.pos.y > selection.pos.y, "{:?}", faces.pos);
        assert!(
            near(construction.pos.x, sketch.pos.x),
            "{:?}",
            construction.pos
        );
        assert!(
            sketch.pos.y < construction.pos.y && construction.pos.y < help.pos.y,
            "{:?}",
            construction.pos
        );
        assert!(selection.pos.x < sketch.pos.x);
        // Two X caps, one on each row's line (within its 22), right of its name.
        let caps = texts(&h, "X");
        assert_eq!(caps.len(), 2);
        for row in [&faces, &construction] {
            let on_line: Vec<&TextShape> = caps
                .iter()
                .filter(|cap| {
                    (middle(cap) - middle(row)).abs() < css::ROW_H / 2.0 && cap.pos.x > row.pos.x
                })
                .collect();
            assert_eq!(on_line.len(), 1, "{:?}", row.galley.text());
        }
    }
}
