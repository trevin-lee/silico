//! The origin planes drawn in the viewport (the approved prototype's `PLANES`,
//! `buildPlanes`, and `VP.setPlanes`, docs/design/m5-ui/js/viewport.js; its hover,
//! interact.js's `startPickPlane` and `planeRow`). Each of XY, XZ, and YZ is the square from
//! (−4, −4) to (46, 46) in its own (U, V) (XY's U is x and V y; XZ's, x and z; YZ's, y and z),
//! filled in [`theme::VIEWPORT_PLANE_FILL`] and outlined in
//! [`theme::VIEWPORT_PLANE_OUTLINE`], [`theme::VIEWPORT_PLANE_WIDTH`] wide; a plane hovered
//! in [`theme::VIEWPORT_PLANE_HOVER_FILL`] and [`theme::VIEWPORT_PLANE_HOVER_OUTLINE`].
//! The renderer draws them among the bodies, translucent, and in no picking
//! (`silico_render::render::Plane`).
//!
//! Which are drawn, one rule for every way a plane shows ([`drawn`]):
//! - while Create Sketch's planes are open (its menu, from the toolbar's button, CREATE's
//!   drop-down, or the canvas's context menu: [`crate::toolbar::planes`]), all three, the
//!   one whose item the pointer is on (or that has the keys' focus) hovered: the prototype's
//!   `startPickPlane` shows all three and lights the one under the pointer, which in the app
//!   is the menu's item for it;
//! - else each whose browser row's eye is open (each opens hidden, as the prototype's,
//!   Fusion's, and SolidWorks' origin planes do), and the one whose browser row the pointer
//!   is on, hovered, its eye open or not (the prototype's `planeRow` shows a plane its row
//!   selects, hidden or not, lit; the app's origin rows don't select, so the pointer on the
//!   row is what lights it, as a timeline item hovered lights its faces).

use eframe::egui;
use silico_core::document::OriginPlane;
use silico_render::render::Plane;

use crate::theme;

/// The origin planes, each with its browser row's name (`crate::tab::Hideable::Origin`),
/// in the browser's order.
pub const ORIGIN_PLANES: [(OriginPlane, &str); 3] = [
    (OriginPlane::Xy, "XY"),
    (OriginPlane::Xz, "XZ"),
    (OriginPlane::Yz, "YZ"),
];

/// The prototype's square on each plane, in its (U, V): from (−4, −4) to (46, 46).
const LOW: f64 = -4.0;
const HIGH: f64 = 46.0;

/// The origin plane whose browser row is named `name`, if one is.
pub fn of_row(name: &str) -> Option<OriginPlane> {
    ORIGIN_PLANES
        .iter()
        .find(|(_, row)| *row == name)
        .map(|(plane, _)| *plane)
}

/// The browser row's name of `plane`.
pub fn row_of(plane: OriginPlane) -> &'static str {
    match plane {
        OriginPlane::Xy => "XY",
        OriginPlane::Xz => "XZ",
        OriginPlane::Yz => "YZ",
    }
}

/// `plane`'s square as the renderer draws it, in the hover's colours if `hovered`.
pub fn plane(plane: OriginPlane, hovered: bool) -> Plane {
    let (u, v) = match plane {
        OriginPlane::Xy => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        OriginPlane::Xz => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        OriginPlane::Yz => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    };
    let side = HIGH - LOW;
    let (fill, outline) = if hovered {
        (
            theme::VIEWPORT_PLANE_HOVER_FILL,
            theme::VIEWPORT_PLANE_HOVER_OUTLINE,
        )
    } else {
        (theme::VIEWPORT_PLANE_FILL, theme::VIEWPORT_PLANE_OUTLINE)
    };
    Plane {
        origin: std::array::from_fn(|k| LOW * (u[k] + v[k])),
        u: u.map(|c| c * side),
        v: v.map(|c| c * side),
        fill,
        outline,
        width: theme::VIEWPORT_PLANE_WIDTH,
    }
}

/// The planes a frame draws, in [`ORIGIN_PLANES`]' order: each `shown` (by its eye) or
/// `hovered`, the hovered one in the hover's colours; or, while Create Sketch's planes are
/// open (`menu`, the plane its item lit, if any), all three, the one lit hovered.
pub fn drawn(
    shown: impl Fn(OriginPlane) -> bool,
    hovered: Option<OriginPlane>,
    menu: Option<Option<OriginPlane>>,
) -> Vec<Plane> {
    let (hovered, all) = match menu {
        Some(lit) => (lit, true),
        None => (hovered, false),
    };
    ORIGIN_PLANES
        .iter()
        .filter(|(each, _)| all || shown(*each) || hovered == Some(*each))
        .map(|(each, _)| plane(*each, hovered == Some(*each)))
        .collect()
}

/// What Create Sketch's planes ([`crate::toolbar::planes`]) said in the pass they were last
/// drawn: the pass, and the plane whose item was lit. Kept in egui's memory, so the one
/// menu of them says it wherever it's drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MenuSaid {
    pass: u64,
    lit: Option<OriginPlane>,
}

fn menu_id() -> egui::Id {
    egui::Id::new("create-sketch-planes")
}

/// Create Sketch's planes drawn this pass, `lit` the plane whose item the pointer is on (or
/// has the keys' focus): said for the viewport ([`menu`]). A plane lit otherwise than the
/// pass before has the next pass asked for, so a viewport drawn before the menu draws it.
pub(crate) fn menu_drawn(ctx: &egui::Context, lit: Option<OriginPlane>) {
    let pass = ctx.cumulative_pass_nr();
    let before = ctx.data(|d| d.get_temp::<MenuSaid>(menu_id()));
    ctx.data_mut(|d| d.insert_temp(menu_id(), MenuSaid { pass, lit }));
    if before.is_none_or(|said| said.lit != lit || said.pass + 1 < pass) {
        ctx.request_repaint();
    }
}

/// Whether Create Sketch's planes are open, as a frame drawn now takes them, and the plane
/// their item lit: drawn this pass or the one before (a menu drawn after the viewport, as
/// the canvas's is, is the pass before's). None once they weren't drawn in either. Its
/// answer `now` is this pass's alone: what [`menu_changed`] compares.
pub fn menu(ctx: &egui::Context) -> Option<Option<OriginPlane>> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<MenuSaid>(menu_id()))
        .filter(|said| said.pass + 1 >= pass)
        .map(|said| said.lit)
}

/// Whether what Create Sketch's planes say now, at the end of the viewport's pass, differs
/// from what its frame drew (`drew`, [`menu`]'s answer when it was built): opened, closed,
/// or another plane lit since. Then the next pass is asked for, so a menu closed or moved
/// after the frame was built (the canvas's, or the toolbar's closing) is drawn as it is.
pub fn menu_changed(ctx: &egui::Context, drew: Option<Option<OriginPlane>>) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let now = ctx
        .data(|d| d.get_temp::<MenuSaid>(menu_id()))
        .filter(|said| said.pass == pass)
        .map(|said| said.lit);
    now != drew
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_plane_is_the_prototypes_square_from_minus_4_to_46_in_its_own_u_and_v() {
        // viewport.js's PLANES and `Geo.rectShape(-4, -4, 46, 46)`, worked by hand: XY's
        // corners (−4, −4, 0), (46, −4, 0), (46, 46, 0), (−4, 46, 0); XZ's in x and z; YZ's
        // in y and z.
        let corners = |p: OriginPlane| plane(p, false).corners();
        assert_eq!(
            corners(OriginPlane::Xy),
            [
                [-4.0, -4.0, 0.0],
                [46.0, -4.0, 0.0],
                [46.0, 46.0, 0.0],
                [-4.0, 46.0, 0.0]
            ]
        );
        assert_eq!(
            corners(OriginPlane::Xz),
            [
                [-4.0, 0.0, -4.0],
                [46.0, 0.0, -4.0],
                [46.0, 0.0, 46.0],
                [-4.0, 0.0, 46.0]
            ]
        );
        assert_eq!(
            corners(OriginPlane::Yz),
            [
                [0.0, -4.0, -4.0],
                [0.0, 46.0, -4.0],
                [0.0, 46.0, 46.0],
                [0.0, -4.0, 46.0]
            ]
        );
    }

    #[test]
    fn a_plane_is_the_prototypes_grey_and_one_hovered_its_blue() {
        // `#9f9f9f` at 0.08 (20 of 255) filled, at 0.6 (153) outlined, 1 px; hovered
        // `#186add` at 0.32 (82), outlined `#4676ff` at 0.6.
        let plain = plane(OriginPlane::Xy, false);
        assert_eq!(plain.fill, [0x9f, 0x9f, 0x9f, 20]);
        assert_eq!(plain.outline, [0x9f, 0x9f, 0x9f, 153]);
        assert_eq!(plain.width, 1.0);
        let lit = plane(OriginPlane::Xy, true);
        assert_eq!(lit.fill, [0x18, 0x6a, 0xdd, 82]);
        assert_eq!(lit.outline, [0x46, 0x76, 0xff, 153]);
        assert_eq!(lit.width, 1.0);
    }

    #[test]
    fn the_planes_drawn_are_those_shown_and_the_one_hovered_or_all_three_for_the_menu() {
        use OriginPlane::{Xy, Xz, Yz};
        let none = |_: OriginPlane| false;
        assert!(drawn(none, None, None).is_empty());
        // XZ's eye open: XZ alone, plain.
        assert_eq!(drawn(|p| p == Xz, None, None), [plane(Xz, false)]);
        // YZ's row hovered, its eye closed: YZ drawn, lit, beside XZ.
        assert_eq!(
            drawn(|p| p == Xz, Some(Yz), None),
            [plane(Xz, false), plane(Yz, true)]
        );
        // XZ hovered and shown: once, lit.
        assert_eq!(drawn(|p| p == Xz, Some(Xz), None), [plane(Xz, true)]);
        // The menu open, nothing lit: all three plain, whatever the eyes and the row.
        assert_eq!(
            drawn(none, Some(Yz), Some(None)),
            [plane(Xy, false), plane(Xz, false), plane(Yz, false)]
        );
        // The menu's XY lit.
        assert_eq!(
            drawn(|p| p == Yz, None, Some(Some(Xy))),
            [plane(Xy, true), plane(Xz, false), plane(Yz, false)]
        );
    }

    #[test]
    fn rows_and_planes_name_each_other() {
        for (plane, row) in ORIGIN_PLANES {
            assert_eq!(of_row(row), Some(plane));
            assert_eq!(row_of(plane), row);
        }
        for other in ["Origin", "X", "Y", "Z", "xy", ""] {
            assert_eq!(of_row(other), None, "{other}");
        }
    }

    #[test]
    fn the_menus_planes_are_open_for_the_pass_they_were_drawn_in_and_the_next() {
        let ctx = egui::Context::default();
        let pass = |ctx: &egui::Context, f: &mut dyn FnMut(&egui::Context)| {
            // Its textures let go, as a painter would (epaint checks they were).
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| f(ui.ctx()));
            output.textures_delta.clear();
        };
        pass(&ctx, &mut |ctx| {
            assert_eq!(menu(ctx), None);
            menu_drawn(ctx, Some(OriginPlane::Xz));
            assert_eq!(menu(ctx), Some(Some(OriginPlane::Xz)));
            assert!(!menu_changed(ctx, Some(Some(OriginPlane::Xz))));
            assert!(menu_changed(ctx, None));
        });
        // The next pass, not drawn again (a canvas's menu, drawn after the viewport): still
        // open as the frame takes it, but changed by the pass's end, so another is asked.
        pass(&ctx, &mut |ctx| {
            assert_eq!(menu(ctx), Some(Some(OriginPlane::Xz)));
            assert!(menu_changed(ctx, Some(Some(OriginPlane::Xz))));
            assert!(!menu_changed(ctx, None));
        });
        // The pass after: closed.
        pass(&ctx, &mut |ctx| assert_eq!(menu(ctx), None));
    }
}
