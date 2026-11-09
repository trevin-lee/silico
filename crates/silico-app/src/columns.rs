//! The window's three columns, the browser's, the canvas's, and the property panel's: each
//! side pane's border dragged to resize it (Trevin, 2026-11-09; docs/design/m5-ui/DESIGN.md,
//! *Layout*), the canvas taking up the difference, and each laid out for the window's width
//! ([`fit`]), the side panes giving back width toward their least before the canvas goes
//! below its own. The widths dragged to are kept across restarts (`crate::pane_widths`).
//!
//! The borders are dragged by handles of the app's own, over the tiles' 1-point gaps, not
//! by `egui_tiles`' splitters: those share a drag with the column beyond the canvas once
//! the canvas is at its least, keep one least for every tile, and even out a split that's
//! double-clicked, where a border here moves its own column alone, by the drag's distance,
//! and stops at the first bound it meets.

use eframe::egui::{self, CursorIcon, Rect, Sense, Ui};

use crate::pane_widths::{BROWSER_MIN_W, PaneWidths, RIGHT_MIN_W, Widths};
use crate::theme::CANVAS_MIN_WIDTH;

/// A side pane, whose column's inner border is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Side {
    /// The browser, at the window's left: its border is its column's right edge.
    Browser,
    /// The property panel, at the window's right: its border is its column's left edge.
    Properties,
}

impl Side {
    /// Its column's least width.
    fn least(self) -> f32 {
        match self {
            Side::Browser => BROWSER_MIN_W,
            Side::Properties => RIGHT_MIN_W,
        }
    }
}

/// Each column's width as laid out in a window, the side panes' with their borders: the
/// browser's, the canvas's, and the property panel's, from the window's left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Laid {
    pub browser: f32,
    pub canvas: f32,
    pub properties: f32,
}

impl Laid {
    /// `side`'s column's width.
    fn of(self, side: Side) -> f32 {
        match side {
            Side::Browser => self.browser,
            Side::Properties => self.properties,
        }
    }
}

/// The columns for a window `width` wide, the side panes `chosen` wide (clamped into their
/// ranges, [`Widths::clamped`]), the canvas the rest:
/// - while the chosen widths leave the canvas at least [`CANVAS_MIN_WIDTH`], each pane as
///   chosen;
/// - else the panes give back what the canvas's least needs past the width, each in
///   proportion to how far it's above its own least, so they come to their leasts together,
///   to whole points (the browser's share rounded, the property panel's the rest);
/// - and once both are at their leasts, the canvas goes below its own, down to nothing, and
///   the property panel runs past the window's right edge, where it's cut off (as the
///   prototype's grid does, `#canvas-wrap`'s `min-width: 0`).
pub(crate) fn fit(chosen: Widths, width: f32) -> Laid {
    let Widths {
        browser,
        properties,
    } = chosen.clamped();
    // NaN's `max` is 0.
    let width = width.max(0.0);
    let over = browser + properties + CANVAS_MIN_WIDTH - width;
    if over <= 0.0 {
        return Laid {
            browser,
            canvas: width - browser - properties,
            properties,
        };
    }
    let browser_room = browser - BROWSER_MIN_W;
    let properties_room = properties - RIGHT_MIN_W;
    let room = browser_room + properties_room;
    if over >= room {
        return Laid {
            browser: BROWSER_MIN_W,
            canvas: (width - BROWSER_MIN_W - RIGHT_MIN_W).max(0.0),
            properties: RIGHT_MIN_W,
        };
    }
    // `room` is more than `over`, which is more than 0. Each share from 0 to its room: the
    // browser's rounded share no more than its room or the whole (a share of a fraction of
    // a point can round up past it), and the rest no more than the property panel's room,
    // which in a window of a fractional width it can pass by a fraction of a point, then
    // the browser's.
    let browser_gives = (over * browser_room / room)
        .round()
        .min(browser_room)
        .min(over);
    let properties_gives = (over - browser_gives).min(properties_room);
    let browser_gives = over - properties_gives;
    Laid {
        browser: browser - browser_gives,
        canvas: CANVAS_MIN_WIDTH,
        properties: properties - properties_gives,
    }
}

/// The width `side`'s column is dragged to: `start`, its width as the drag began, moved by
/// `travel`, the pointer's way right since its press (the browser's border, its column's
/// right edge, widening it; the property panel's, its left edge, narrowing it), to a whole
/// point; no wider than leaves the canvas [`CANVAS_MIN_WIDTH`] in a window `width` wide
/// beside `other`, the other column as laid, and no narrower than its least, which wins
/// where the two cross.
pub(crate) fn dragged(side: Side, start: f32, travel: f32, other: f32, width: f32) -> f32 {
    let wanted = match side {
        Side::Browser => start + travel,
        Side::Properties => start - travel,
    };
    let most = (width - other - CANVAS_MIN_WIDTH).floor();
    wanted.round().min(most).max(side.least())
}

/// A border's drag under way: its pane's, and its column's width as it began.
#[derive(Debug, Clone, Copy)]
struct Drag {
    side: Side,
    start: f32,
}

/// The side panes' widths, as the person chose them by dragging their borders (the
/// defaults till then, or what the kept file holds), and the drag under way.
pub(crate) struct Columns {
    chosen: Widths,
    drag: Option<Drag>,
    /// Whether a border has been dragged: the file's widths, read after, aren't taken.
    dragged: bool,
    kept: Option<PaneWidths>,
}

impl Columns {
    /// The prototype's widths, kept nowhere.
    pub(crate) fn new() -> Self {
        Columns {
            chosen: Widths::default(),
            drag: None,
            dragged: false,
            kept: None,
        }
    }

    /// The widths kept in `kept`, taken once it's read, each drag's written to it.
    pub(crate) fn keeping(&mut self, kept: PaneWidths) {
        self.kept = Some(kept);
    }

    /// The widths as chosen.
    pub(crate) fn chosen(&self) -> Widths {
        self.chosen
    }

    /// Where the widths are kept, if anywhere.
    pub(crate) fn kept(&self) -> Option<&PaneWidths> {
        self.kept.as_ref()
    }

    /// The kept widths taken, if the file has been read since the last poll and no border
    /// has been dragged before it was (the person's drag wins, and is written at its end).
    pub(crate) fn poll(&mut self) {
        if let Some(read) = self.kept.as_mut().and_then(PaneWidths::poll)
            && !self.dragged
        {
            self.chosen = read;
        }
    }

    /// The thread keeping the widths asks `ctx` for a frame when it has read them.
    pub(crate) fn waking(&self, ctx: &egui::Context) {
        if let Some(kept) = &self.kept {
            kept.waking(ctx);
        }
    }

    /// The columns for a window `width` wide ([`fit`]).
    pub(crate) fn laid(&self, width: f32) -> Laid {
        fit(self.chosen, width)
    }

    /// Each side pane's border's handle, over the columns laid as `laid` in `area` (the
    /// tiles' rect: its left the browser's, its width the window's): the border's 1 point
    /// and egui's grab radius at each side of it, the resize cursor shown while it's hovered
    /// or dragged. A drag moves its column by the pointer's way from its press, clamped
    /// ([`dragged`]), the other held as laid, from the next frame; its widths are kept as it
    /// ends. Drawn after the panes, so it's on top of what they sense beside the border.
    pub(crate) fn handles(&mut self, ui: &Ui, area: Rect, laid: Laid) {
        let grab = ui.style().interaction.resize_grab_radius_side;
        for side in [Side::Browser, Side::Properties] {
            let x = match side {
                Side::Browser => area.left() + laid.browser - BORDER,
                Side::Properties => area.left() + laid.browser + laid.canvas,
            };
            let border = Rect::from_x_y_ranges(x..=x + BORDER, area.y_range());
            // Clicks too, so a press on the border never reaches the canvas under it; never
            // focusable, so Tab skips it.
            let response = ui.interact(
                border.expand2(egui::vec2(grab, 0.0)),
                ui.id().with(("column-border", side)),
                Sense::CLICK | Sense::DRAG,
            );
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if response.drag_started() {
                self.drag = Some(Drag {
                    side,
                    start: laid.of(side),
                });
                // The other column held as it's shown, so this one moves by the drag alone.
                self.chosen = Widths {
                    browser: laid.browser,
                    properties: laid.properties,
                };
            }
            let pointer = ui.input(|input| {
                input
                    .pointer
                    .press_origin()
                    .zip(input.pointer.interact_pos())
            });
            if response.dragged()
                && let Some(drag) = self.drag.filter(|drag| drag.side == side)
                && let Some((from, to)) = pointer
            {
                let other = match side {
                    Side::Browser => laid.properties,
                    Side::Properties => laid.browser,
                };
                let width = dragged(side, drag.start, to.x - from.x, other, area.width());
                match side {
                    Side::Browser => self.chosen.browser = width,
                    Side::Properties => self.chosen.properties = width,
                }
                self.dragged = true;
                ui.ctx().request_repaint();
            }
            if response.drag_stopped() && self.drag.take().is_some() {
                if let Some(kept) = &self.kept {
                    kept.save(self.chosen);
                }
                ui.ctx().request_repaint();
            }
        }
    }
}

/// A side pane's border (`#browser`'s `border-right`, `#right`'s `border-left`): the tiles'
/// gap (`crate::panes`), the last point of the browser's column and the first of the
/// property panel's.
pub(crate) const BORDER: f32 = 1.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(browser: f32, properties: f32) -> Widths {
        Widths {
            browser,
            properties,
        }
    }

    fn laid(browser: f32, canvas: f32, properties: f32) -> Laid {
        Laid {
            browser,
            canvas,
            properties,
        }
    }

    #[test]
    fn a_window_with_room_lays_the_panes_out_as_chosen_and_the_canvas_the_rest() {
        // By hand: 1280 − 248 − 296 = 736; 1600 − 248 − 296 = 1056; at 884, 884 − 544 =
        // 340, the canvas's least exactly; 1280 − 400 − 300 = 580.
        let prototype = widths(248.0, 296.0);
        assert_eq!(fit(prototype, 1280.0), laid(248.0, 736.0, 296.0));
        assert_eq!(fit(prototype, 1600.0), laid(248.0, 1056.0, 296.0));
        assert_eq!(fit(prototype, 884.0), laid(248.0, 340.0, 296.0));
        assert_eq!(fit(widths(400.0, 300.0), 1280.0), laid(400.0, 580.0, 300.0));
        // A fractional width's fraction is the canvas's.
        assert_eq!(fit(prototype, 1280.5), laid(248.0, 736.5, 296.0));
    }

    #[test]
    fn a_narrow_window_takes_from_the_panes_in_proportion_to_their_room_first() {
        // The leasts: the browser 200, the property panel 232, the canvas 340; 772 in all.
        // At 800, from 248 and 296: over = 248 + 296 + 340 − 800 = 84; the rooms 48 and 64,
        // 112 in all; the browser gives 84 · 48 / 112 = 36, the property panel 48: 212,
        // 340, 248.
        let prototype = widths(248.0, 296.0);
        assert_eq!(fit(prototype, 800.0), laid(212.0, 340.0, 248.0));
        // At 850: over 34; the browser 34 · 48 / 112 = 14.57, 15, the property panel 19.
        assert_eq!(fit(prototype, 850.0), laid(233.0, 340.0, 277.0));
        // At 883, a point under the room they need: the browser 1 · 48 / 112 = 0.43, 0; the
        // property panel gives the point.
        assert_eq!(fit(prototype, 883.0), laid(248.0, 340.0, 295.0));
        // At 773, a point over the leasts: over 111, the browser 111 · 48 / 112 = 47.57,
        // 48, the property panel 63.
        assert_eq!(fit(prototype, 773.0), laid(200.0, 340.0, 233.0));
        // One at its least already: the other gives it all. From 200 and 400 at 900: over
        // 40, all the property panel's: 360. From 500 and 232: over 172, all the browser's.
        assert_eq!(fit(widths(200.0, 400.0), 900.0), laid(200.0, 340.0, 360.0));
        assert_eq!(fit(widths(500.0, 232.0), 900.0), laid(328.0, 340.0, 232.0));
        // Wide panes from a wider window: from 600 and 400 at 1000, over 340; the rooms 400
        // and 168, 568; the browser 340 · 400 / 568 = 239.44, 239, the property panel 101.
        assert_eq!(fit(widths(600.0, 400.0), 1000.0), laid(361.0, 340.0, 299.0));
    }

    #[test]
    fn past_the_panes_leasts_the_canvas_goes_below_its_own_down_to_nothing() {
        // The leasts' 772 exactly: both panes at their leasts, the canvas at its.
        let prototype = widths(248.0, 296.0);
        assert_eq!(fit(prototype, 772.0), laid(200.0, 340.0, 232.0));
        // Under it, the canvas alone gives: 700 − 432 = 268, 432 − 432 = 0.
        assert_eq!(fit(prototype, 700.0), laid(200.0, 268.0, 232.0));
        assert_eq!(fit(prototype, 432.0), laid(200.0, 0.0, 232.0));
        // Narrower than the panes' leasts: the canvas nothing, the panes at their leasts,
        // the property panel past the window's edge; and a width that isn't one, as 0.
        for width in [300.0, 0.0, -50.0, f32::NAN] {
            assert_eq!(fit(prototype, width), laid(200.0, 0.0, 232.0), "{width}");
        }
        // Chosen widths out of range are clamped first: 0, 225, and a NaN to the leasts, an
        // infinity to the widest, 8192: 1280 − 432 = 848; 9000 − 8192 − 232 = 576.
        assert_eq!(
            fit(widths(0.0, f32::NAN), 1280.0),
            laid(200.0, 848.0, 232.0)
        );
        assert_eq!(
            fit(widths(f32::INFINITY, 225.0), 9000.0),
            laid(8192.0, 576.0, 232.0)
        );
    }

    #[test]
    fn a_fractional_window_keeps_each_pane_at_its_least_or_more() {
        // At 772.5: over = 248 + 296 + 340 − 772.5 = 111.5, under the room 112; the browser
        // 111.5 · 48 / 112 = 47.79, 48, its room whole; the property panel 63.5 of its 64.
        let at = fit(widths(248.0, 296.0), 772.5);
        assert_eq!(at, laid(200.0, 340.0, 232.5));
        // At 883.5, half a point short: the browser 0.5 · 48 / 112 = 0.21, 0; the property
        // panel the half.
        assert_eq!(fit(widths(248.0, 296.0), 883.5), laid(248.0, 340.0, 295.5));
        // From 201 and 296 at 772.5: over 64.5, the rooms 1 and 64; the browser 64.5 / 65 =
        // 0.99, 1; the property panel 63.5.
        assert_eq!(fit(widths(201.0, 296.0), 772.5), laid(200.0, 340.0, 232.5));
        // From 248 and 233 at 772.5: over 48.5, rooms 48 and 1; the browser 48.5 · 48 / 49 =
        // 47.51, 48, its room; the property panel the 0.5 left, of its 1.
        assert_eq!(fit(widths(248.0, 233.0), 772.5), laid(200.0, 340.0, 232.5));
        // A share of a fraction of a point that rounds up past the whole: from 600 and 233
        // at 1172.25, over = 600 + 233 + 340 − 1172.25 = 0.75; the rooms 400 and 1; the
        // browser 0.75 · 400 / 401 = 0.748, 1 rounded, but no more than the 0.75 there is.
        assert_eq!(
            fit(widths(600.0, 233.0), 1172.25),
            laid(599.25, 340.0, 233.0)
        );
    }

    #[test]
    fn every_width_lays_out_to_the_window_and_no_pane_under_its_least() {
        // Over a sweep of windows and chosen widths, by the rules alone: the three sum to
        // the window (while the panes' leasts fit in it), each pane at or under its choice
        // and at or over its least, and the canvas at its least while a pane gives.
        for browser in [200.0, 201.0, 248.0, 333.0, 600.0, 2000.0] {
            for properties in [232.0, 233.0, 296.0, 401.0, 900.0] {
                for tenth in 4320..=30000 {
                    let width = tenth as f32 / 10.0;
                    let at = fit(widths(browser, properties), width);
                    let sum = at.browser + at.canvas + at.properties;
                    let what = format!("{browser} and {properties} at {width}: {at:?}");
                    assert!((sum - width).abs() < 1e-3, "{what}");
                    assert!(at.browser >= 200.0 && at.browser <= browser, "{what}");
                    assert!(
                        at.properties >= 232.0 && at.properties <= properties,
                        "{what}"
                    );
                    assert!(at.canvas >= 0.0, "{what}");
                    if at.browser < browser || at.properties < properties {
                        let leasts = at.browser == 200.0 && at.properties == 232.0;
                        assert!(at.canvas == 340.0 || leasts, "{what}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_drag_moves_its_column_by_its_distance_between_its_bounds() {
        // By hand, in a 1280 window from the prototype's 248 and 296: the browser's border
        // 80 right, 328; 48 left, 200, its least; 60 left, 188, so 200. Its most beside
        // 296: 1280 − 296 − 340 = 644.
        assert_eq!(dragged(Side::Browser, 248.0, 80.0, 296.0, 1280.0), 328.0);
        assert_eq!(dragged(Side::Browser, 248.0, -48.0, 296.0, 1280.0), 200.0);
        assert_eq!(dragged(Side::Browser, 248.0, -60.0, 296.0, 1280.0), 200.0);
        assert_eq!(dragged(Side::Browser, 248.0, 396.0, 296.0, 1280.0), 644.0);
        assert_eq!(dragged(Side::Browser, 248.0, 397.0, 296.0, 1280.0), 644.0);
        // The property panel's border left widens it: 60 left, 356; 64 right, 232, its
        // least; 100 right, 196, so 232. Its most beside 248: 1280 − 248 − 340 = 692.
        assert_eq!(
            dragged(Side::Properties, 296.0, -60.0, 248.0, 1280.0),
            356.0
        );
        assert_eq!(dragged(Side::Properties, 296.0, 64.0, 248.0, 1280.0), 232.0);
        assert_eq!(
            dragged(Side::Properties, 296.0, 100.0, 248.0, 1280.0),
            232.0
        );
        assert_eq!(
            dragged(Side::Properties, 296.0, -1000.0, 248.0, 1280.0),
            692.0
        );
        // To a whole point: 10.4 is 10, 10.5 is 11.
        assert_eq!(dragged(Side::Browser, 248.0, 10.4, 296.0, 1280.0), 258.0);
        assert_eq!(dragged(Side::Browser, 248.0, 10.5, 296.0, 1280.0), 259.0);
        assert_eq!(
            dragged(Side::Properties, 296.0, -10.5, 248.0, 1280.0),
            307.0
        );
        // A fractional window's most is whole: 1280.5 − 296 − 340 = 644.5, 644.
        assert_eq!(dragged(Side::Browser, 248.0, 500.0, 296.0, 1280.5), 644.0);
        // A window with no room past the leasts: the least, whatever the drag.
        for travel in [-100.0, 0.0, 100.0] {
            assert_eq!(dragged(Side::Browser, 200.0, travel, 232.0, 700.0), 200.0);
            assert_eq!(
                dragged(Side::Properties, 232.0, travel, 200.0, 700.0),
                232.0
            );
        }
    }
}
