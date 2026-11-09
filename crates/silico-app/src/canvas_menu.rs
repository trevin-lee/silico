//! The canvas's context menu (M5.7; docs/design/m5.0-app.md, section 9's *Selection*): what a
//! right click on the canvas opens in selection mode, as the approved prototype's `onContext`
//! (js/interact.js) builds it, drawn as every menu is (`crate::menu`). Over an element, what a
//! click there would select (the selection's hover): headed by it and its kind ("Face6 ·
//! Face"), then *Select Other…* with how many are under the pointer, Edit and the feature
//! that made it, a separator, and Hide and its body. Over nothing: Fit (F6), Home view
//! (Home), *Select Other…*, a separator, and Clear selection (Esc). *Select Other…* opens
//! the list of what's under the pointer (`crate::select_other`, which holds this menu while
//! it's open); every other item is done by the tab ([`CanvasAct`]) as its command is
//! elsewhere: Fit, Home view, and Clear selection the registry's (`crate::shortcuts`, their
//! caps its), Hide as a body's eye in the browser hides it (with no key's cap: H, the
//! registry's Hide, hides the feature selected, not the body under the pointer), and Edit as
//! the feature menus' Edit feature (`crate::feature_menu`, off with its reason where that
//! is).
//!
//! Left out of the prototype's items, none a command the app has yet:
//! - SEAM(#351): Sketch on this face and Create Sketch, sketch mode's;
//! - SEAM: Look At (Ctrl+8), once the view can turn square to a face (`crate::shortcuts`'
//!   seam);
//! - SEAM: Fillet, an edge's, whose dialog opens from MODIFY (`crate::toolbar`, through the
//!   app's remembered inputs), not from a command of the registry;
//! - SEAM: Copy name ("as agents write it"), no command of the registry copies a name.

use eframe::egui::{self, Response, Ui, Vec2};
use silico_core::document::FeatureId;

use crate::icons::Icon;
use crate::menu;
use crate::shortcuts::Command;

/// *Select Other…*, as the prototype labels it.
pub const SELECT_OTHER: &str = "Select Other…";

/// What *Select Other…* says while the view's meshes aren't of the model the selection is
/// read against: nothing was cast.
pub const UNREAD: &str = "the model's meshes aren't shown yet";

/// What *Select Other…* says when a body's boxes to cast on can't be made (`crate::boxes`:
/// the thread that makes them couldn't be started, or ended): nothing was cast.
pub const UNCAST: &str = "what's under the pointer can't be found";

/// One of the menu's items: *Select Other…*, which the menu does itself, or one the tab
/// does once it's chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasItem {
    SelectOther,
    Act(CanvasAct),
}

/// What a chosen item asks the tab to do, the menu closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasAct {
    /// Edit feature, of the feature that made the element under the pointer.
    Edit(FeatureId),
    /// Hide, of the body under the pointer (by the feature that ends its chain).
    Hide(FeatureId),
    Fit,
    Home,
    ClearSelection,
}

/// An item as the menu shows it: what it is, its icon, its words, its dim words after them,
/// its keys' caps, and why it's off, if it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub item: CanvasItem,
    pub icon: Option<Icon>,
    pub label: String,
    pub sub: Option<String>,
    pub caps: Vec<String>,
    pub off: Option<String>,
}

impl Shown {
    fn of(item: CanvasItem, icon: Option<Icon>, label: impl Into<String>) -> Shown {
        Shown {
            item,
            icon,
            label: label.into(),
            sub: None,
            caps: Vec::new(),
            off: None,
        }
    }

    /// It with `command`'s caps, as the platform names them.
    fn keys(self, command: Command, ctx: &egui::Context) -> Shown {
        Shown {
            caps: command.caps(ctx),
            ..self
        }
    }

    /// As `crate::menu` draws it, lit or not.
    fn line(&self, lit: bool) -> menu::Item<'_> {
        menu::Item {
            icon: self.icon,
            label: &self.label,
            sub: self.sub.as_deref(),
            caps: &self.caps,
            off: self.off.as_deref(),
            danger: false,
            lit,
        }
    }

    /// What a screen reader (and a test) finds it by: its words, its dim words, its keys.
    pub fn said(&self, ctx: &egui::Context) -> String {
        self.line(false).said(ctx)
    }
}

/// What's under the pointer at the right click, as the menu says it: the element a click
/// there would select, by its words ("Face6") and its kind ("Face"); the feature that made
/// it, by its ID and its name; and its body, by the feature that ends its chain and its
/// name. None over nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Under {
    pub label: String,
    pub kind: &'static str,
    pub maker: FeatureId,
    pub maker_name: String,
    pub body: FeatureId,
    pub body_name: String,
}

/// What the menu says of the rest: how many are under the pointer (*Select Other…*'s dim
/// words) and whether there's any to list; why Edit can't open the maker's dialog, if it
/// can't; and whether anything is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub count: String,
    pub listable: bool,
    pub edit_off: Option<String>,
    pub selected: bool,
}

/// The canvas's context menu: its head, if it's over an element, and its items, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasMenu {
    pub head: Option<String>,
    pub items: Vec<Shown>,
}

impl CanvasMenu {
    /// The menu over `under` (or over nothing), as the prototype's `onContext` builds it,
    /// its caps as `ctx`'s platform names them.
    pub fn new(under: Option<&Under>, context: &Context, ctx: &egui::Context) -> CanvasMenu {
        let mut select_other = Shown::of(CanvasItem::SelectOther, Some(Icon::Target), SELECT_OTHER);
        select_other.sub = Some(context.count.clone());
        if !context.listable {
            select_other.off = Some("Nothing is under the pointer".to_owned());
        }
        match under {
            Some(under) => {
                let mut edit = Shown::of(
                    CanvasItem::Act(CanvasAct::Edit(under.maker.clone())),
                    Some(Icon::Edit),
                    format!("Edit {}", under.maker_name),
                );
                edit.off = context.edit_off.clone();
                // No key's cap: H hides the feature selected, not the body under the
                // pointer this hides.
                let hide = Shown::of(
                    CanvasItem::Act(CanvasAct::Hide(under.body.clone())),
                    Some(Icon::EyeOff),
                    format!("Hide {}", under.body_name),
                );
                CanvasMenu {
                    head: Some(format!("{} · {}", under.label, under.kind)),
                    items: vec![select_other, edit, hide],
                }
            }
            None => {
                let fit = Shown::of(
                    CanvasItem::Act(CanvasAct::Fit),
                    Some(Icon::Fit),
                    Command::Fit.label(),
                )
                .keys(Command::Fit, ctx);
                let home = Shown::of(
                    CanvasItem::Act(CanvasAct::Home),
                    Some(Icon::Home),
                    Command::Home.label(),
                )
                .keys(Command::Home, ctx);
                let mut clear = Shown::of(
                    CanvasItem::Act(CanvasAct::ClearSelection),
                    None,
                    Command::ClearSelection.label(),
                )
                .keys(Command::ClearSelection, ctx);
                if !context.selected {
                    clear.off = Some("Nothing is selected".to_owned());
                }
                CanvasMenu {
                    head: None,
                    items: vec![fit, home, select_other, clear],
                }
            }
        }
    }

    /// Whether a separator comes before its item `at` (the prototype's `'sep'`: before Hide,
    /// and before Clear selection).
    fn separated(&self, at: usize) -> bool {
        self.items.get(at).is_some_and(|shown| {
            matches!(
                shown.item,
                CanvasItem::Act(CanvasAct::Hide(_) | CanvasAct::ClearSelection)
            )
        })
    }

    /// Whether its item `at` can be chosen.
    pub fn on(&self, at: usize) -> bool {
        self.items.get(at).is_some_and(|shown| shown.off.is_none())
    }

    /// The item lit `by` steps on from `lit` (none lit: the first step forward lights the
    /// first, back the last), round its ends, passing over those that are off; none if none
    /// is on.
    pub fn step(&self, lit: Option<usize>, by: i64) -> Option<usize> {
        let on: Vec<usize> = (0..self.items.len()).filter(|&at| self.on(at)).collect();
        let count = i64::try_from(on.len()).ok().filter(|&count| count > 0)?;
        let from = match lit.and_then(|lit| on.iter().position(|&at| at == lit)) {
            Some(place) => i64::try_from(place).ok()?,
            None if by >= 0 => -1,
            None => count,
        };
        // Taken round first, so no step overflows.
        let to = (from + by.rem_euclid(count)).rem_euclid(count);
        on.get(usize::try_from(to).ok()?).copied()
    }

    /// Its lines' width: its widest line's, its head's too, and a menu's least at least
    /// (`crate::menu::width`).
    fn width(&self, ui: &Ui) -> f32 {
        let lines: Vec<menu::Item<'_>> = self.items.iter().map(|shown| shown.line(false)).collect();
        menu::width(ui, self.head.as_deref(), &lines, 0.0)
    }

    /// It drawn, its item `lit` lit: which item the pointer is over, and which was clicked.
    pub fn draw(&self, ui: &mut Ui, lit: Option<usize>) -> (Option<usize>, Option<usize>) {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let width = self.width(ui);
        if let Some(head) = &self.head {
            menu::head(ui, head, width);
        }
        let (mut pointed, mut clicked) = (None, None);
        for (at, shown) in self.items.iter().enumerate() {
            if self.separated(at) {
                menu::separator(ui, width);
            }
            let response: Response = menu::item(ui, &shown.line(lit == Some(at)), width);
            if shown.off.is_none() && response.hovered() {
                pointed = Some(at);
            }
            if response.clicked() {
                clicked = Some(at);
            }
        }
        (pointed, clicked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fid(id: &str) -> FeatureId {
        FeatureId::new(id).unwrap()
    }

    fn under() -> Under {
        Under {
            label: "Face6".to_owned(),
            kind: "Face",
            maker: fid("k7f2qx3m"),
            maker_name: "Block".to_owned(),
            body: fid("p0000001"),
            body_name: "Plate".to_owned(),
        }
    }

    fn context(listable: bool, selected: bool) -> Context {
        Context {
            count: if listable {
                "4 under the pointer".to_owned()
            } else {
                "nothing under the pointer".to_owned()
            },
            listable,
            edit_off: None,
            selected,
        }
    }

    fn said(menu: &CanvasMenu, ctx: &egui::Context) -> Vec<(String, bool)> {
        menu.items
            .iter()
            .map(|shown| (shown.said(ctx), shown.off.is_none()))
            .collect()
    }

    #[test]
    fn over_an_element_and_over_nothing_its_items_are_the_prototypes_in_its_order() {
        // js/interact.js's `onContext`, of the items the app has commands for: over an
        // element its head (the element and its kind), Select Other… with its count, Edit
        // and the maker, and Hide and the body, with no key (H hides the feature selected,
        // not the body under the pointer); over nothing Fit with F6, Home view
        // with Home, Select Other…, and Clear selection with Esc (off with nothing selected).
        let ctx = egui::Context::default();
        ctx.set_os(egui::os::OperatingSystem::Nix);
        let over = CanvasMenu::new(Some(&under()), &context(true, false), &ctx);
        assert_eq!(over.head.as_deref(), Some("Face6 · Face"));
        assert_eq!(
            said(&over, &ctx),
            [
                ("Select Other… 4 under the pointer".to_owned(), true),
                ("Edit Block".to_owned(), true),
                ("Hide Plate".to_owned(), true),
            ]
        );
        assert_eq!(
            over.items
                .iter()
                .map(|s| s.item.clone())
                .collect::<Vec<_>>(),
            [
                CanvasItem::SelectOther,
                CanvasItem::Act(CanvasAct::Edit(fid("k7f2qx3m"))),
                CanvasItem::Act(CanvasAct::Hide(fid("p0000001"))),
            ]
        );
        assert!(over.items[2].caps.is_empty(), "{:?}", over.items[2]);
        assert!(!over.separated(1) && over.separated(2));
        let off = CanvasMenu::new(None, &context(false, false), &ctx);
        assert_eq!(off.head, None);
        assert_eq!(
            said(&off, &ctx),
            [
                ("Fit F6".to_owned(), true),
                ("Home view Home".to_owned(), true),
                ("Select Other… nothing under the pointer".to_owned(), false),
                ("Clear selection Esc".to_owned(), false),
            ]
        );
        assert!(off.separated(3) && !off.separated(2));
        let selected = CanvasMenu::new(None, &context(true, true), &ctx);
        assert!(selected.on(2) && selected.on(3));
        // Edit off says why, as the feature menus' Edit feature does.
        let mut busy = context(true, false);
        busy.edit_off = Some("Close the open dialog first".to_owned());
        let over = CanvasMenu::new(Some(&under()), &busy, &ctx);
        assert_eq!(
            over.items[1].off.as_deref(),
            Some("Close the open dialog first")
        );
    }

    #[test]
    fn a_step_goes_round_the_items_that_are_on_passing_over_those_off() {
        let ctx = egui::Context::default();
        // Over nothing, nothing selected, nothing under the pointer: Fit and Home on, Select
        // Other… and Clear selection off.
        let menu = CanvasMenu::new(None, &context(false, false), &ctx);
        assert_eq!(menu.step(None, 1), Some(0));
        assert_eq!(menu.step(None, -1), Some(1));
        assert_eq!(menu.step(Some(1), 1), Some(0));
        assert_eq!(menu.step(Some(0), -1), Some(1));
        assert_eq!(menu.step(Some(0), i64::MAX), Some(1));
        assert_eq!(menu.step(Some(0), i64::MIN), Some(0));
        // Everything on: round all four.
        let all = CanvasMenu::new(None, &context(true, true), &ctx);
        assert_eq!(all.step(Some(3), 1), Some(0));
        assert_eq!(all.step(Some(1), 2), Some(3));
        // An item lit that's off (it went off while lit) steps from none.
        assert_eq!(menu.step(Some(2), 1), Some(0));
        // None on: none lit.
        let mut none = CanvasMenu::new(None, &context(false, false), &ctx);
        for shown in &mut none.items {
            shown.off = Some("off".to_owned());
        }
        assert_eq!(none.step(None, 1), None);
        assert_eq!(none.step(Some(0), -1), None);
    }
}
