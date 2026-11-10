//! A double-click counted on its widget's own clicks (#452). egui counts the pointer's clicks,
//! not a widget's: a release within its double-click time (0.3 s) of the click before is a
//! double-click, but one within twice that of the click before last is a triple, wherever
//! those clicks were. So a double-click on a browser row less than 0.6 s after a click on an
//! eye, another row, or a timeline item reached three, `Response::double_clicked` was false,
//! and Edit didn't open; and a middle double-click on the canvas just after a left click
//! anywhere didn't fit. Here a click of a button on a widget within egui's double-click time
//! of that button's click on that widget before is a double-click, whatever came on other
//! widgets, or of other buttons, between. The pair's count starts again after it, so a third
//! quick click isn't a second double-click, as a platform's triple click isn't.

use eframe::egui::{Event, Id, PointerButton, Response};

/// Whether the widget `response` answers for was double-clicked this frame by its own clicks
/// of the primary button (the module's rule), a pointer's, not a key that clicks the widget.
pub fn double_clicked(response: &Response) -> bool {
    double_clicked_by(response, PointerButton::Primary)
}

/// Whether the widget `response` answers for was double-clicked this frame by its own clicks
/// of `button` (the module's rule). The release of each of that button's clicks on it this
/// frame is counted, and the widget's last unpaired click of each button is kept in egui's
/// memory between frames. Two clicks a frame apart, or both in one frame (as a test, or a fast
/// enough hand, gives them), are a double-click alike.
pub fn double_clicked_by(response: &Response, button: PointerButton) -> bool {
    if !response.clicked_by(button) {
        return false;
    }
    let ctx = &response.ctx;
    let delay = ctx.options(|options| options.input_options.max_double_click_delay);
    let (now, releases) = ctx.input(|input| {
        let releases = input
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::PointerButton {
                        pos,
                        button: released,
                        pressed: false,
                        ..
                    } if *released == button && response.interact_rect.contains(*pos)
                )
            })
            .count();
        (input.time, releases)
    });
    let id = last_click(response.id, button);
    let mut last: Option<f64> = ctx.data(|data| data.get_temp(id));
    let mut double = false;
    // At least the click egui saw, though its release's place wasn't in this frame's events.
    for _ in 0..releases.max(1) {
        match last {
            Some(at) if now - at < delay => {
                double = true;
                last = None;
            }
            _ => last = Some(now),
        }
    }
    ctx.data_mut(|data| match last {
        Some(at) => {
            data.insert_temp(id, at);
        }
        None => data.remove::<f64>(id),
    });
    double
}

/// Where a widget's last unpaired click of `button` is kept in egui's memory.
fn last_click(widget: Id, button: PointerButton) -> Id {
    // `PointerButton` isn't `Hash`; its discriminant tells the buttons apart.
    widget.with(("silico_last_click", button as u8))
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Context, Modifiers, Pos2, RawInput, Rect, Sense, pos2, vec2};

    use super::*;

    /// Two widgets side by side: a row, (0, 0) to (100, 20), and an eye, (120, 0) to (140, 20).
    const ROW: Pos2 = Pos2::new(50.0, 10.0);
    const EYE: Pos2 = Pos2::new(130.0, 10.0);

    /// A frame at `time` with `events`: whether the row and the eye were double-clicked by the
    /// primary button, by this module's rule and by egui's own.
    fn frame(ctx: &Context, time: f64, events: Vec<Event>) -> [(bool, bool); 2] {
        framed(ctx, time, events, PointerButton::Primary)
    }

    /// [`frame`], of `button`'s double-clicks.
    fn framed(
        ctx: &Context,
        time: f64,
        events: Vec<Event>,
        button: PointerButton,
    ) -> [(bool, bool); 2] {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 100.0))),
            time: Some(time),
            events,
            ..RawInput::default()
        };
        let mut seen = [(false, false); 2];
        let mut output = ctx.run_ui(input, |ui| {
            for (k, (name, rect)) in [
                ("row", Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0))),
                (
                    "eye",
                    Rect::from_min_max(pos2(120.0, 0.0), pos2(140.0, 20.0)),
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let response = ui.interact(rect, Id::new(name), Sense::click());
                seen[k] = (
                    double_clicked_by(&response, button),
                    response.double_clicked_by(button),
                );
            }
        });
        // Nothing draws these frames: their textures are let go.
        output.textures_delta.clear();
        seen
    }

    fn press(at: Pos2, pressed: bool) -> Event {
        press_of(PointerButton::Primary, at, pressed)
    }

    fn press_of(button: PointerButton, at: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: Modifiers::default(),
        }
    }

    /// A click at `at`: the pointer there, then its press and release, all in one frame at
    /// `time`, as kittest's double-click gives them.
    fn click(at: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(at), press(at, true), press(at, false)]
    }

    #[test]
    fn a_double_click_on_a_row_just_after_a_click_on_an_eye_is_the_rows_double_click() {
        // #452: the eye at 0.0, then the row's two clicks at 0.45, inside egui's 0.6 s for a
        // triple. egui's count reaches three, so its double-click is false; the row's own two
        // clicks are a double-click.
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert_eq!(frame(&ctx, 0.1, click(EYE))[1], (false, false));
        let mut twice = click(ROW);
        twice.extend([press(ROW, true), press(ROW, false)]);
        let row = frame(&ctx, 0.45, twice)[0];
        assert_eq!(row, (true, false), "the row's, then egui's");
    }

    #[test]
    fn two_clicks_a_frame_apart_are_a_double_click_within_its_time_and_not_past_it() {
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert!(!frame(&ctx, 1.0, click(ROW))[0].0);
        // 0.2 s on: a double-click, as egui's own is.
        assert_eq!(frame(&ctx, 1.2, click(ROW))[0], (true, true));
        // Then 2.0 and 2.4: 0.4 s apart, past egui's 0.3, so none.
        assert!(!frame(&ctx, 2.0, click(ROW))[0].0);
        assert!(!frame(&ctx, 2.4, click(ROW))[0].0);
    }

    #[test]
    fn a_third_quick_click_starts_again_and_is_no_second_double_click() {
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert!(!frame(&ctx, 1.0, click(ROW))[0].0);
        assert!(frame(&ctx, 1.1, click(ROW))[0].0);
        assert!(!frame(&ctx, 1.2, click(ROW))[0].0, "the third click");
        assert!(
            frame(&ctx, 1.3, click(ROW))[0].0,
            "the fourth, the third's pair"
        );
    }

    #[test]
    fn a_click_on_another_widget_is_neither_ones_double_click() {
        // The row at 1.0, the eye at 1.1: each a first click, none a double-click.
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert_eq!(frame(&ctx, 1.0, click(ROW))[0], (false, false));
        assert_eq!(frame(&ctx, 1.1, click(EYE)), [(false, false); 2]);
    }

    #[test]
    fn a_click_on_another_widget_in_the_same_frame_isnt_counted_as_its() {
        // The eye's click and the row's in one frame at 1.0: one click on each, neither a
        // double-click. The row's next click, 0.1 s on, is the pair of its one.
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        let mut both = click(EYE);
        both.extend(click(ROW));
        assert!(
            !frame(&ctx, 1.0, both)[0].0,
            "the eye's click isn't the row's"
        );
        assert!(frame(&ctx, 1.1, click(ROW))[0].0);
    }

    #[test]
    fn each_buttons_clicks_are_counted_apart() {
        // A left click on the eye at 0.1, then a middle double-click on the row at 0.45: the
        // row's middle double-click, where egui's count of the pointer's clicks reaches three.
        // And a left click on the row, then a middle one 0.1 s on: neither button's
        // double-click, where egui, counting the pointer's clicks of any button, calls the
        // middle one a double-click.
        let ctx = Context::default();
        let middle = PointerButton::Middle;
        frame(&ctx, 0.0, Vec::new());
        frame(&ctx, 0.1, click(EYE));
        let twice = vec![
            Event::PointerMoved(ROW),
            press_of(middle, ROW, true),
            press_of(middle, ROW, false),
            press_of(middle, ROW, true),
            press_of(middle, ROW, false),
        ];
        assert_eq!(framed(&ctx, 0.45, twice, middle)[0], (true, false));
        frame(&ctx, 2.0, click(ROW));
        let once = vec![press_of(middle, ROW, true), press_of(middle, ROW, false)];
        assert_eq!(framed(&ctx, 2.1, once.clone(), middle)[0], (false, true));
        assert!(
            !frame(&ctx, 2.2, once.clone())[0].0,
            "a middle click isn't the left's pair"
        );
        // A left click and a middle one on the row in one frame at 4.0: the middle's one
        // click, not a pair, where egui's count again calls it a middle double-click.
        let mut mixed = click(ROW);
        mixed.extend(once);
        assert_eq!(framed(&ctx, 4.0, mixed, middle)[0], (false, true));
    }

    #[test]
    fn a_key_that_clicks_a_focused_widget_twice_is_no_double_click() {
        // Enter on the focused row clicks it (egui's fake primary click), not the pointer.
        let ctx = Context::default();
        let enter = || {
            vec![Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            }]
        };
        frame(&ctx, 0.0, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(Id::new("row")));
        frame(&ctx, 0.05, Vec::new());
        assert!(!frame(&ctx, 1.0, enter())[0].0);
        assert!(!frame(&ctx, 1.1, enter())[0].0);
    }
}
