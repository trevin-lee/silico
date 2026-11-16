//! The command search (docs/design/m5-ui/DESIGN.md, *Agents as equal clients* and *egui
//! mapping*; its `styles.css`'s `.palette-modal` rules, `js/app.js`'s `paletteItems` and
//! `renderPalette`, and `js/interact.js`'s keys): `S` opens a box over a dimmed window, its
//! field taking the keyboard, listing the commands of the one registry (`crate::shortcuts`),
//! each with its icon, its name, its group, and its keys, narrowed by what's typed. ↑ and ↓
//! move a highlight, Enter or a click runs a command, Esc or a click on the backdrop closes
//! it. A command that can't run now is listed off, saying why where its button or menu item
//! says it, and runs from here as its keys or its button run it (`App::run_searched`).
//!
//! It's an `Area` anchored at the window's top centre, as DESIGN.md maps it, over a backdrop
//! of its own: no modal, so nothing of the window under it is drawn otherwise.

use eframe::egui::{
    self, Align, FontId, Key, Modifiers, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
    Vec2, WidgetInfo, WidgetType, accesskit::Role, pos2, vec2,
};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::menu;
use crate::shortcuts::{self, Command, Entry, REGISTRY};
use crate::theme;

/// The box's name, and its field's (the prototype's `aria-label`s) and placeholder.
pub(crate) const TITLE: &str = "Search commands";

/// What the list says when nothing matches (the prototype's `.mh`, in its capitals).
pub(crate) const NOTHING_FOUND: &str = "No command matches";

/// The foot's words at its right.
pub(crate) const MCP: &str = "Agents reach the same commands over MCP";

/// The most rows listed (`paletteItems`'s `slice(0, 40)`).
pub(crate) const MOST: usize = 40;

/// The registry's commands the search leaves out (`paletteItems`'s `ok`, `cancel`, `orbit`,
/// `pan`, and `zoom`: a dialog's keys and the pointer's gestures; its `selectOther` and
/// `sNoRel` aren't in the app's registry).
pub(crate) const LEFT_OUT: [Command; 5] = [
    Command::Ok,
    Command::Cancel,
    Command::Orbit,
    Command::Pan,
    Command::Zoom,
];

/// The foot's parts: each one's caps, then its words (`.pm-foot`'s spans).
const FOOT: [(&[&str], &str); 3] = [
    (&["↑", "↓"], " move"),
    (&["↵"], " run"),
    (&["Esc"], " close"),
];

/// Whether the search finds `entry` for `query`, its case aside: the query in its name, its
/// group's, or its description (`paletteItems`'s filter); an empty query finds every one.
pub(crate) fn finds(entry: &Entry, query: &str) -> bool {
    let query = query.to_lowercase();
    query.is_empty()
        || [
            entry.label,
            entry.group.name(),
            entry.description.unwrap_or_default(),
        ]
        .iter()
        .any(|text| text.to_lowercase().contains(&query))
}

/// The commands listed for `query`, in the registry's order: those it finds, but
/// [`LEFT_OUT`], the first [`MOST`] of them.
pub(crate) fn listed(query: &str) -> Vec<&'static Entry> {
    REGISTRY
        .iter()
        .filter(|entry| !LEFT_OUT.contains(&entry.command) && finds(entry, query))
        .take(MOST)
        .collect()
}

/// Whether a command can run now, and if not, why, where its button or menu item says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Can {
    Run,
    Off(Option<String>),
}

/// The search while it's open.
#[derive(Debug, Default)]
pub(crate) struct Search {
    /// What's typed in its field.
    pub query: String,
    /// The row the keys light, in the list for the query.
    highlight: usize,
    /// Whether the highlight was moved by a key, so its row is scrolled into view.
    moved: bool,
}

impl Search {
    /// The row the keys light, in a list of `rows`.
    pub(crate) fn highlight(&self, rows: usize) -> usize {
        self.highlight.min(rows.saturating_sub(1))
    }
}

/// What the person asked of the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asked {
    None,
    Close,
    Run(Command),
}

/// The keyboard let go as the search closes: its field had it, and a key after is a
/// command's again.
pub(crate) fn let_go(ctx: &egui::Context) {
    ctx.memory_mut(|memory| {
        if let Some(focused) = memory.focused() {
            memory.surrender_focus(focused);
        }
    });
}

/// The search's keys this frame, taken before its field reads them: Esc closes it, ↑ and ↓
/// move the highlight (clamped to the list), and Enter runs the highlighted command, if it
/// can run (`interact.js`'s `S.palette` keys). `can` says whether each can.
pub(crate) fn keys(ui: &mut Ui, search: &mut Search, can: &dyn Fn(Command) -> Can) -> Asked {
    if shortcuts::take(ui, Command::Cancel) {
        return Asked::Close;
    }
    let (down, up, enter) = ui.input_mut(|input| {
        (
            input.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
            input.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
            input.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    let listed = listed(&search.query);
    let last = listed.len().saturating_sub(1);
    let mut highlight = search.highlight;
    for _ in 0..down {
        highlight = (highlight + 1).min(last);
    }
    for _ in 0..up {
        highlight = highlight.saturating_sub(1);
    }
    if down + up > 0 {
        search.moved = true;
    }
    search.highlight = highlight;
    if enter
        && let Some(entry) = listed.get(highlight)
        && can(entry.command) == Can::Run
    {
        return Asked::Run(entry.command);
    }
    Asked::None
}

/// The box's place in `screen`: [`theme::SEARCH_WIDTH`] wide (the window's width at most),
/// centred across it, [`theme::SEARCH_TOP`] from its top, as high as its parts with `rows`
/// rows listed.
pub(crate) fn place(screen: Rect, rows: usize) -> Rect {
    let width = theme::SEARCH_WIDTH.min(screen.width());
    let height =
        2.0 * theme::BORDER_WIDTH + theme::SEARCH_INPUT_HEIGHT + list_height(rows) + foot_height();
    Rect::from_min_size(
        pos2(
            screen.center().x - width / 2.0,
            screen.top() + theme::SEARCH_TOP,
        ),
        vec2(width, height),
    )
}

/// The list's height with `rows` rows (`.pm-list`): its padding and its rows, or its line
/// saying nothing matches, at most [`theme::SEARCH_LIST_MAX_HEIGHT`].
fn list_height(rows: usize) -> f32 {
    let inside = if rows == 0 {
        theme::MENU_HEAD_TOP + theme::MENU_LINE_HEIGHT + theme::MENU_HEAD_BOTTOM
    } else {
        rows as f32 * theme::SEARCH_ROW_HEIGHT
    };
    (2.0 * theme::SEARCH_LIST_PADDING + inside).min(theme::SEARCH_LIST_MAX_HEIGHT)
}

/// The foot's height (`.pm-foot`): its rule, its padding, and a line of the body's.
fn foot_height() -> f32 {
    let padding = theme::SEARCH_FOOT_PADDING;
    theme::BORDER_WIDTH
        + f32::from(padding.top)
        + theme::BODY_LINE_HEIGHT
        + f32::from(padding.bottom)
}

/// The search over the window: its backdrop, then its box. What was asked: a click on the
/// backdrop closes it, a click on a row that can run runs it. `can` says whether each can.
pub(crate) fn show(
    ctx: &egui::Context,
    search: &mut Search,
    can: &dyn Fn(Command) -> Can,
) -> Asked {
    let screen = ctx.content_rect();
    let backdrop = egui::Area::new(egui::Id::new("command-search-backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .show(ctx, |ui| {
            let response = ui.allocate_rect(screen, Sense::click());
            ui.painter()
                .rect_filled(screen, 0.0, theme::SEARCH_BACKDROP);
            response.clicked()
        })
        .inner;
    let area = egui::Area::new(egui::Id::new("command-search"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .show(ctx, |ui| boxed(ui, screen, search, can));
    // Over its backdrop, whichever was last touched.
    ctx.move_to_top(area.response.layer_id);
    match area.inner {
        Asked::None if backdrop => Asked::Close,
        asked => asked,
    }
}

/// The box (`.palette-modal`) in `screen`: its field's row, its list, and its foot.
fn boxed(ui: &mut Ui, screen: Rect, search: &mut Search, can: &dyn Fn(Command) -> Can) -> Asked {
    ui.scope_builder(UiBuilder::new().id_salt("command-search"), |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        // Its frame, painted once its height is known.
        let frame = ui.painter().add(Shape::Noop);
        let width = theme::SEARCH_WIDTH.min(screen.width());
        let top = screen.top() + theme::SEARCH_TOP;
        let left = screen.center().x - width / 2.0;
        let inner_left = left + theme::BORDER_WIDTH;
        let inner_right = left + width - theme::BORDER_WIDTH;
        let field_row = Rect::from_min_max(
            pos2(inner_left, top + theme::BORDER_WIDTH),
            pos2(
                inner_right,
                top + theme::BORDER_WIDTH + theme::SEARCH_INPUT_HEIGHT,
            ),
        );
        let changed = field(ui, field_row, search);
        if changed {
            search.highlight = 0;
        }
        let listed = listed(&search.query);
        let rect = place(screen, listed.len());
        ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
            node.set_role(Role::Dialog);
            node.set_label(TITLE);
            node.set_bounds(crate::panes::bounds(rect));
        });
        // The whole box is the search's, so a click on it reaches nothing under it.
        ui.allocate_rect(rect, Sense::hover());
        let shapes = vec![
            Shape::from(theme::SEARCH_SHADOW.as_shape(rect, theme::SEARCH_CORNER_RADIUS)),
            Shape::rect_filled(rect, theme::SEARCH_CORNER_RADIUS, theme::SEARCH_BG),
            Shape::rect_stroke(
                rect,
                theme::SEARCH_CORNER_RADIUS,
                Stroke::new(theme::BORDER_WIDTH, theme::SEARCH_BORDER),
                StrokeKind::Inside,
            ),
        ];
        ui.painter().set(frame, Shape::Vec(shapes));
        let list_top = top + theme::BORDER_WIDTH + theme::SEARCH_INPUT_HEIGHT;
        let list = Rect::from_min_max(
            pos2(inner_left, list_top),
            pos2(inner_right, list_top + list_height(listed.len())),
        );
        let asked = rows(ui, list, &listed, search, changed, can);
        foot(
            ui,
            Rect::from_min_max(
                list.left_bottom(),
                pos2(inner_right, rect.bottom() - theme::BORDER_WIDTH),
            ),
        );
        asked
    })
    .inner
}

/// The field's row (`.pm-input`) in `row`, its rule in its last point: the search icon, then
/// the field, which keeps the keyboard while the search is open. Whether what's typed
/// changed.
fn field(ui: &mut Ui, row: Rect, search: &mut Search) -> bool {
    let line = row.bottom() - theme::BORDER_WIDTH / 2.0;
    ui.painter().hline(
        row.x_range(),
        line,
        Stroke::new(theme::BORDER_WIDTH, theme::SEARCH_RULE),
    );
    let content = row.with_max_y(row.bottom() - theme::BORDER_WIDTH);
    let middle = content.center().y;
    let icon = Rect::from_min_size(
        pos2(
            content.left() + theme::SEARCH_INPUT_PADDING,
            middle - ICON_SIZE / 2.0,
        ),
        Vec2::splat(ICON_SIZE),
    );
    Icon::Search
        .image(IconSize::Small)
        .tint(theme::SEARCH_ICON)
        .paint_at(ui, icon);
    let place = Rect::from_min_max(
        pos2(icon.right() + theme::SEARCH_INPUT_GAP, content.top()),
        pos2(
            content.right() - theme::SEARCH_INPUT_PADDING,
            content.bottom(),
        ),
    );
    let font = FontId::proportional(theme::SEARCH_INPUT_SIZE);
    let edit = egui::TextEdit::singleline(&mut search.query)
        .id_salt("command-search-field")
        .font(font.clone())
        .text_color(theme::SEARCH_INPUT_TEXT)
        .hint_text(
            egui::RichText::new(TITLE)
                .font(font)
                .color(theme::TEXT_SUBDUED),
        )
        .frame(egui::Frame::NONE)
        .vertical_align(Align::Center)
        .desired_width(place.width());
    let typed = ui.put(place, edit);
    ui.ctx()
        .accesskit_node_builder(typed.id, |node| node.set_label(TITLE));
    // The keyboard stays in the field (the prototype focuses it as it opens): a click on a
    // row or on the box takes it nowhere else.
    if !typed.has_focus() {
        typed.request_focus();
    }
    typed.changed()
}

/// The list (`.pm-list`) in `list`: a row for each of `listed`, the highlighted one lit, each
/// that can't run off, scrolling past its height; or that nothing matches. What a click
/// asked.
fn rows(
    ui: &mut Ui,
    list: Rect,
    listed: &[&'static Entry],
    search: &mut Search,
    changed: bool,
    can: &dyn Fn(Command) -> Can,
) -> Asked {
    let inside = list.shrink(theme::SEARCH_LIST_PADDING);
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt("command-search-list")
            .max_rect(inside)
            .layout(egui::Layout::top_down(Align::Min)),
    );
    child.spacing_mut().item_spacing = Vec2::ZERO;
    child
        .ctx()
        .accesskit_node_builder(child.unique_id(), |node| {
            node.set_role(Role::ListBox);
            node.set_label(TITLE);
            node.set_bounds(crate::panes::bounds(list));
        });
    if listed.is_empty() {
        menu::head(&mut child, NOTHING_FOUND, inside.width());
        return Asked::None;
    }
    let highlight = search.highlight(listed.len());
    let moved = std::mem::take(&mut search.moved);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("command-search-scroll")
        .max_height(inside.height())
        .auto_shrink([false, false]);
    if changed {
        // A new list starts at its top, as the prototype's is drawn anew.
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll
        .show(&mut child, |ui| {
            let mut asked = Asked::None;
            let ctx = ui.ctx().clone();
            for (at, entry) in listed.iter().enumerate() {
                let caps = entry.command.caps(&ctx);
                let can = can(entry.command);
                let reason = match &can {
                    Can::Run => None,
                    Can::Off(reason) => reason.clone(),
                };
                let item = menu::Item {
                    icon: Some(entry.icon.unwrap_or(Icon::Chevron)),
                    label: entry.label,
                    sub: Some(entry.group.name()),
                    caps: &caps,
                    off: reason.as_deref(),
                    danger: false,
                    lit: at == highlight,
                    checked: None,
                };
                let runs = can == Can::Run;
                let response = ui
                    .add_enabled_ui(runs, |ui| {
                        menu::item_sized(ui, &item, inside.width(), theme::SEARCH_ROW_HEIGHT)
                    })
                    .inner;
                ctx.accesskit_node_builder(response.id, |node| {
                    node.set_role(Role::ListBoxOption);
                    node.set_selected(at == highlight);
                });
                if moved && at == highlight {
                    response.scroll_to_me(None);
                }
                if runs && response.clicked() {
                    asked = Asked::Run(entry.command);
                }
            }
            asked
        })
        .inner
}

/// The foot (`.pm-foot`) in `rect`, under its rule: each key's caps and what it does, then
/// at its right that agents reach the same commands.
fn foot(ui: &mut Ui, rect: Rect) {
    let padding = theme::SEARCH_FOOT_PADDING;
    ui.painter().hline(
        rect.x_range(),
        rect.top() + theme::BORDER_WIDTH / 2.0,
        Stroke::new(theme::BORDER_WIDTH, theme::SEARCH_RULE),
    );
    let middle =
        rect.top() + theme::BORDER_WIDTH + f32::from(padding.top) + theme::BODY_LINE_HEIGHT / 2.0;
    let mut x = rect.left() + f32::from(padding.left);
    for (caps, words) in FOOT {
        let caps: Vec<String> = caps.iter().map(|cap| (*cap).to_owned()).collect();
        // Each key's caps in a `.keys` of its own, side by side.
        let mut wide = 0.0;
        for cap in &caps {
            let one = std::slice::from_ref(cap);
            let width = crate::start::key_caps_width(ui, one);
            crate::start::key_caps(ui, pos2(x + wide + width, middle), one, false);
            wide += width;
        }
        let galley = foot_text(ui, words);
        let at = pos2(x + wide, middle - galley.size().y / 2.0);
        let part = Rect::from_min_max(
            pos2(x, middle - theme::KEY_CAP_HEIGHT / 2.0),
            pos2(at.x + galley.size().x, middle + theme::KEY_CAP_HEIGHT / 2.0),
        );
        x = part.right() + theme::SEARCH_FOOT_GAP;
        ui.painter().galley(at, galley, theme::SEARCH_FOOT_TEXT);
        label(ui, part, &format!("{}{words}", caps.concat()));
    }
    let galley = foot_text(ui, MCP);
    let at = pos2(
        rect.right() - f32::from(padding.right) - galley.size().x,
        middle - galley.size().y / 2.0,
    );
    let part = Rect::from_min_size(at, galley.size());
    ui.painter().galley(at, galley, theme::SEARCH_FOOT_TEXT);
    label(ui, part, MCP);
}

/// `text` in the foot's type.
fn foot_text(ui: &Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::proportional(theme::SEARCH_FOOT_SIZE),
        theme::SEARCH_FOOT_TEXT,
    )
}

/// A label for a screen reader over `rect`, saying `said`.
fn label(ui: &mut Ui, rect: Rect, said: &str) {
    ui.interact(
        rect,
        ui.id().with(("command-search-foot", said)),
        Sense::hover(),
    )
    .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, said));
}

#[cfg(test)]
mod tests {
    use eframe::egui::Pos2;

    use super::*;

    fn commands(query: &str) -> Vec<Command> {
        listed(query).iter().map(|entry| entry.command).collect()
    }

    #[test]
    fn the_filter_finds_a_name_a_group_or_a_description_its_case_aside() {
        // By name, any case: "Fit" alone has "fit" in its name, its group (View), or its
        // description ("The whole model in view").
        assert_eq!(commands("fit"), [Command::Fit]);
        assert_eq!(commands("FIT"), [Command::Fit]);
        assert_eq!(commands("fIt"), [Command::Fit]);
        // By group, and by description: "rebuild" is the Rebuild group's name, so its five,
        // and in Suppress's description ("left out of the rebuild") and Finish Sketch's
        // ("Keeps your changes and rebuilds"), each in the registry's order.
        assert_eq!(
            commands("rebuild"),
            [
                Command::Suppress,
                Command::Rebuild,
                Command::RebuildAll,
                Command::Reload,
                Command::Stop,
                Command::WhatsWrong,
                Command::FinishSketch
            ]
        );
        // By group alone: "dialogs" is only the Dialogs group's, whose OK and Cancel are left
        // out, so nothing.
        assert_eq!(commands("dialogs"), []);
        // "help": the Help group's Keyboard shortcuts alone.
        assert_eq!(commands("help"), [Command::Shortcuts]);
        // By description alone: "cache" is in Rebuild everything's ("Every feature,
        // ignoring the cache"), in no name or group.
        assert_eq!(commands("cache"), [Command::RebuildAll]);
        assert_eq!(commands("CACHE"), [Command::RebuildAll]);
        // Not by keys: "f6" is Fit's key, in no name, group, or description.
        assert_eq!(commands("f6"), []);
        assert_eq!(commands("no such thing"), []);
    }

    #[test]
    fn it_leaves_out_ok_cancel_and_the_pointers_gestures() {
        // `paletteItems`: ok, cancel, orbit, pan, zoom (and select-other and the no-relation
        // hold, which the registry hasn't).
        // "ok": OK left out; Look At's name holds it ("lOOK").
        assert_eq!(commands("ok"), [Command::LookAt]);
        assert_eq!(commands("cancel"), []);
        assert_eq!(commands("orbit"), []);
        assert_eq!(commands("pan"), []);
        // "zoom": Zoom to the pointer, left out.
        assert_eq!(commands("zoom"), []);
        for left_out in LEFT_OUT {
            assert!(
                !listed("").iter().any(|entry| entry.command == left_out),
                "{left_out:?}"
            );
        }
        // "view": Home view's name, the View group's fourteen less Orbit, Pan, and Zoom, and
        // Fit's description ("The whole model in view").
        assert_eq!(
            commands("view"),
            [
                Command::Fit,
                Command::Home,
                Command::Front,
                Command::Back,
                Command::Left,
                Command::Right,
                Command::Top,
                Command::Bottom,
                Command::Isometric,
                Command::LookAt
            ]
        );
    }

    #[test]
    fn at_most_forty_are_listed_the_first_in_the_registrys_order() {
        // The registry's 48 less the five left out are 43: the first 40 listed, so the last
        // three (Construction, Finish Sketch, Keyboard shortcuts) aren't until a query finds
        // them.
        assert_eq!(REGISTRY.len(), 48);
        let all = listed("");
        assert_eq!(all.len(), 40);
        assert_eq!(all[0].command, Command::New);
        assert_eq!(all[39].command, Command::Dimension);
        for missing in [
            Command::Construction,
            Command::FinishSketch,
            Command::Shortcuts,
        ] {
            assert!(!all.iter().any(|entry| entry.command == missing));
        }
        assert_eq!(commands("finish"), [Command::FinishSketch]);
        // "e" is in 43's names, groups, or descriptions or more: still 40.
        assert_eq!(listed("e").len(), 40);
    }

    #[test]
    fn the_box_is_520_wide_96_from_the_top_centred_and_as_high_as_its_parts() {
        // styles.css's `.palette-modal`: 520 wide, its border in it, 96 down, `left: 50%;
        // transform: translateX(-50%)`. A 1280 window: (1280 − 520) / 2 = 380.
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
        // One row: its 1 border, the field's 38 (its rule in it), the list's 4 + 28 + 4,
        // the foot's 1 rule, 6, a 16 line, and 6, and the 1 border: 105.
        assert_eq!(
            place(screen, 1),
            Rect::from_min_size(pos2(380.0, 96.0), vec2(520.0, 105.0))
        );
        // Eleven rows: 4 + 308 + 4 = 316, under 340: 1 + 38 + 316 + 29 + 1 = 385.
        assert_eq!(place(screen, 11).height(), 385.0);
        // Twelve and more: 4 + 336 + 4 = 344, held to 340: 1 + 38 + 340 + 29 + 1 = 409.
        assert_eq!(place(screen, 12).height(), 409.0);
        assert_eq!(place(screen, 40).height(), 409.0);
        // None: the line saying so (`.mh`, 6 + 16 + 2) in its padding: 4 + 24 + 4 = 32.
        assert_eq!(place(screen, 0).height(), 1.0 + 38.0 + 32.0 + 29.0 + 1.0);
        // A window narrower than the box: the box its width, from its left.
        let narrow = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 600.0));
        assert_eq!(place(narrow, 1).x_range(), egui::Rangef::new(0.0, 400.0));
    }

    #[test]
    fn the_highlight_is_held_to_the_list() {
        let search = Search {
            highlight: 7,
            ..Search::default()
        };
        assert_eq!(search.highlight(10), 7);
        assert_eq!(search.highlight(3), 2);
        assert_eq!(search.highlight(0), 0);
    }
}
