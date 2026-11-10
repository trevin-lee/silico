//! A double-click counted on its widget's own clicks (#452). egui counts the pointer's clicks,
//! not a widget's: a release within its double-click time (0.3 s) and click distance (6
//! points) of the click before is a double-click, but one within twice that time of the click
//! before last is a triple, whatever widget, or button, those clicks were on. So a
//! double-click on a browser row less than 0.6 s after a click on an eye, another row, or a
//! timeline item reached three, `Response::double_clicked` was false, and Edit didn't open;
//! a middle double-click on the canvas just after a left click anywhere didn't fit; and a
//! double-click on a sketch's dimension just after a click on the Sketch Palette didn't open
//! its box. Here each widget's clicks of each button are counted apart ([`Count`]), by egui's
//! rule: a click within its double-click time and click distance of that widget's click of
//! that button before is a double-click, unless it's a triple (within twice the time of the
//! one before that), whatever came on other widgets, or of other buttons, between.

use eframe::egui::{Event, Id, InputOptions, PointerButton, Pos2, Response};

/// One widget's clicks of one button, counted as egui counts the pointer's: when and where
/// its last click was, and when the one before it was.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Count {
    last: Option<(f64, Pos2)>,
    last_last: Option<f64>,
}

impl Count {
    /// A click at `at`, at `time`: whether it's a double-click, egui's count of two. That's
    /// within `options`' double-click time of the last click and within its click distance of
    /// where that was, but not a triple: a third quick click, within twice that time of the
    /// click before last (and as near the last), isn't a double-click.
    pub fn click(&mut self, at: Pos2, time: f64, options: &InputOptions) -> bool {
        let near = self.last.is_none_or(|(_, last)| {
            last.distance_sq(at) < options.max_click_dist * options.max_click_dist
        });
        let delay = options.max_double_click_delay;
        let double = near && self.last.is_some_and(|(last, _)| time - last < delay);
        let triple = near && self.last_last.is_some_and(|last| time - last < delay * 2.0);
        self.last_last = self.last.map(|(last, _)| last);
        self.last = Some((time, at));
        double && !triple
    }
}

/// Whether the widget `response` answers for was double-clicked this frame by its own clicks
/// of the primary button (the module's rule), a pointer's, not a key that clicks the widget.
/// As [`double_clicked_by`], it's to be asked once a frame for each widget, whatever else
/// decides what a double-click does.
pub fn double_clicked(response: &Response) -> bool {
    double_clicked_by(response, PointerButton::Primary)
}

/// Whether the widget `response` answers for was double-clicked this frame by its own clicks
/// of `button` (the module's rule). Each of that button's releases on it this frame is
/// counted, where it was, and the widget's count of each button is kept in egui's memory
/// between frames. Two clicks a frame apart, or both in one frame (as a test, or a fast enough
/// hand, gives them), are a double-click alike.
///
/// It counts as it answers, so it's to be asked once a frame for each widget, whatever else
/// decides what a double-click does: asked twice, one click would count twice; not asked, a
/// click would go uncounted.
pub fn double_clicked_by(response: &Response, button: PointerButton) -> bool {
    if !response.clicked_by(button) {
        return false;
    }
    let ctx = &response.ctx;
    let options = ctx.options(|options| options.input_options);
    let (now, mut releases, last_anywhere) = ctx.input(|input| {
        let mut on = Vec::new();
        let mut anywhere = None;
        for event in &input.events {
            if let Event::PointerButton {
                pos,
                button: released,
                pressed: false,
                ..
            } = *event
                && released == button
            {
                anywhere = Some(pos);
                if response.interact_rect.contains(pos) {
                    on.push(pos);
                }
            }
        }
        (input.time, on, anywhere.or(input.pointer.interact_pos()))
    });
    // egui gives a click to the nearest widget within its interact radius of the pointer when
    // none is under it, so the click it saw can have its release just outside the widget:
    // then that release, the frame's last of the button, is the click's.
    if releases.is_empty() {
        releases.push(last_anywhere.unwrap_or(response.interact_rect.center()));
    }
    let id = count_id(response.id, button);
    let mut count: Count = ctx.data(|data| data.get_temp(id)).unwrap_or_default();
    let mut double = false;
    for at in releases {
        double |= count.click(at, now, &options);
    }
    ctx.data_mut(|data| data.insert_temp(id, count));
    double
}

/// Where a widget's count of `button`'s clicks is kept in egui's memory.
fn count_id(widget: Id, button: PointerButton) -> Id {
    // `PointerButton` isn't `Hash`; its discriminant tells the buttons apart.
    widget.with(("silico_clicks", button as u8))
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Context, Modifiers, RawInput, Rect, Sense, pos2, vec2};

    use super::*;

    /// Three widgets: a row, (0, 0) to (100, 20), with its eye inside it at its right end, (80,
    /// 0) to (100, 20), drawn over it, as a browser row's is; and another widget beside them,
    /// (120, 0) to (140, 20).
    const ROW: Pos2 = Pos2::new(40.0, 10.0);
    const EYE: Pos2 = Pos2::new(90.0, 10.0);
    const OTHER: Pos2 = Pos2::new(130.0, 10.0);

    /// A frame at `time` with `events`: whether the row, the eye, and the other widget were
    /// double-clicked by the primary button, each by this module's rule and by egui's own.
    fn frame(ctx: &Context, time: f64, events: Vec<Event>) -> [(bool, bool); 3] {
        framed(ctx, time, events, PointerButton::Primary)
    }

    /// [`frame`], of `button`'s double-clicks.
    fn framed(
        ctx: &Context,
        time: f64,
        events: Vec<Event>,
        button: PointerButton,
    ) -> [(bool, bool); 3] {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 100.0))),
            time: Some(time),
            events,
            ..RawInput::default()
        };
        let mut seen = [(false, false); 3];
        let mut output = ctx.run_ui(input, |ui| {
            for (k, (name, rect)) in [
                ("row", Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 20.0))),
                (
                    "eye",
                    Rect::from_min_max(pos2(80.0, 0.0), pos2(100.0, 20.0)),
                ),
                (
                    "other",
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

    /// A click at `at`: the pointer there, then its press and release, all in one frame, as
    /// kittest's double-click gives them.
    fn click(at: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(at), press(at, true), press(at, false)]
    }

    #[test]
    fn a_double_click_on_a_row_just_after_a_click_on_its_eye_is_the_rows_double_click() {
        // #452: the eye's click at 0.1, then the row's two clicks at 0.45, 0.35 s after it,
        // inside egui's 0.6 s for a triple. egui's count of the pointer's clicks reaches
        // three, so its double-click is false; the row's own two clicks are a double-click.
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
        // The 2.4 click is the one counted from: 2.65 is its pair (0.25 s on, and 0.65 s
        // after 2.0, past a triple's 0.6).
        assert_eq!(frame(&ctx, 2.65, click(ROW))[0], (true, true));
    }

    #[test]
    fn a_third_and_a_fourth_quick_click_are_triples_and_no_double_click_as_egui_counts_them() {
        // 0.1 s apart: single, double, then a triple each (within 0.6 s of the click two
        // before), never a double-click, by this rule and egui's alike.
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        let counted: Vec<(bool, bool)> = [1.0, 1.1, 1.2, 1.3]
            .into_iter()
            .map(|time| frame(&ctx, time, click(ROW))[0])
            .collect();
        assert_eq!(
            counted,
            [(false, false), (true, true), (false, false), (false, false)]
        );
    }

    #[test]
    fn two_quick_clicks_a_click_distance_apart_are_no_double_click() {
        // egui's click distance, 6 points: two clicks on the row 0.1 s apart, 50 points apart,
        // are two clicks; 5.9 points apart, a double-click; each as egui's own has it.
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert!(!frame(&ctx, 1.0, click(pos2(10.0, 10.0)))[0].0);
        assert_eq!(frame(&ctx, 1.1, click(pos2(60.0, 10.0)))[0], (false, false));
        assert!(!frame(&ctx, 2.0, click(pos2(10.0, 10.0)))[0].0);
        assert_eq!(frame(&ctx, 2.1, click(pos2(15.9, 10.0)))[0], (true, true));
    }

    #[test]
    fn a_click_on_another_widget_is_neither_ones_double_click() {
        // The row at 1.0, the other widget at 1.1: each a first click, none a double-click.
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert_eq!(frame(&ctx, 1.0, click(ROW))[0], (false, false));
        assert_eq!(frame(&ctx, 1.1, click(OTHER)), [(false, false); 3]);
    }

    #[test]
    fn a_click_on_another_widget_in_the_same_frame_isnt_counted_as_its() {
        // The eye's click, inside the row, and the row's in one frame at 1.0: the eye's
        // release is on the row too, but 50 points from the row's, so no pair: no
        // double-click. (Two clicks on two widgets in one frame come only from a test.)
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        let mut both = click(EYE);
        both.extend(click(ROW));
        assert!(
            !frame(&ctx, 1.0, both)[0].0,
            "the eye's click isn't the row's pair"
        );
        // The row's click at 3.0, then the other widget's and the row's in one frame at 3.1:
        // the other's release, off the row, isn't counted, so the row's is its click's pair.
        assert!(!frame(&ctx, 3.0, click(ROW))[0].0);
        let mut both = click(OTHER);
        both.extend(click(ROW));
        assert!(frame(&ctx, 3.1, both)[0].0, "the other's click is no row's");
    }

    #[test]
    fn a_click_just_outside_a_widget_that_egui_gives_it_is_counted_where_it_was() {
        // 3 points below the row, where no widget is: egui gives the click to the row, the
        // nearest within its interact radius (5 points), and its release is off the row.
        // Two such clicks 0.1 s apart are the row's double-click, as egui's own is.
        let below = pos2(40.0, 23.0);
        let ctx = Context::default();
        frame(&ctx, 0.0, Vec::new());
        assert!(!frame(&ctx, 1.0, click(below))[0].0);
        assert_eq!(frame(&ctx, 1.1, click(below))[0], (true, true));
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

    #[test]
    fn a_count_is_egui_s_by_time_and_place() {
        // egui's defaults: 0.3 s and 6 points. Exactly representable ends: 0.0 and 0.3
        // aren't a double; at 0.0, 0.35, and 0.6, the third is a double, not a triple
        // (0.3 · 2.0 is exactly 0.6, not within it).
        let options = InputOptions::default();
        assert_eq!(
            (options.max_double_click_delay, options.max_click_dist),
            (0.3, 6.0)
        );
        let at = pos2(100.0, 100.0);
        let clicks = |times: &[f64]| -> Vec<bool> {
            let mut count = Count::default();
            times
                .iter()
                .map(|&time| count.click(at, time, &options))
                .collect()
        };
        assert_eq!(clicks(&[0.0, 0.3]), [false, false]);
        assert_eq!(clicks(&[0.0, 0.29]), [false, true]);
        assert_eq!(clicks(&[0.0, 0.35, 0.6]), [false, false, true]);
        // 6 points off, no double; 5.9, a double.
        let mut count = Count::default();
        assert!(!count.click(at, 0.0, &options));
        assert!(!count.click(pos2(106.0, 100.0), 0.1, &options));
        let mut count = Count::default();
        assert!(!count.click(at, 0.0, &options));
        assert!(count.click(pos2(105.9, 100.0), 0.1, &options));
    }
}
