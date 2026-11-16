//! Toasts (the prototype's `toast`, `js/app.js`; `#toasts` and `.toast` in `styles.css`;
//! DESIGN.md's *Toasts*): short words over the canvas's bottom right, several at once,
//! stacked in a column with the newest last, each with its icon (info, or check for
//! something done), each taken down [`theme::TOAST_SECONDS`] after it was shown, by egui's
//! clock. One saying what one shown already says, with its icon (a key held, its repeats
//! each asking again), isn't stacked again: that one's clock starts again.
//!
//! Each shows at once, as the prototype's does, so what it says is read the frame it comes,
//! and fades out over the last egui `animation_time` of its time (the time egui's own
//! transitions take: 1/12 s by default, none when it's 0), an opacity worked out from its age
//! ([`opacity`]), so its time up is the prototype's. A frame is asked for at once only while
//! one fades, else when the next fade begins or, with no fade, when the first is taken down
//! ([`next_frame`]), and none once none is shown.

use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText};

use crate::icons::{ICON_SIZE, Icon, IconSize};
use crate::theme;

/// A toast's icon, as the prototype's `toast(text, ic)` names it: those the app's toasts use.
/// The prototype's `'warn'` and its class `err` wait for a message of the app's that the
/// prototype gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `'info'`, the prototype's default.
    Info,
    /// `'check'`: something done.
    Check,
}

impl Kind {
    /// Its icon.
    pub fn icon(self) -> Icon {
        match self {
            Kind::Info => Icon::Info,
            Kind::Check => Icon::Check,
        }
    }

    /// What its icon is called in the accessibility tree.
    pub fn alt_text(self) -> &'static str {
        match self {
            Kind::Info => "Info",
            Kind::Check => "Done",
        }
    }
}

/// What a toast says: its words and its icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
    pub kind: Kind,
}

impl Toast {
    /// `text` with the info icon, the prototype's default.
    pub fn info(text: impl Into<String>) -> Self {
        Toast {
            text: text.into(),
            kind: Kind::Info,
        }
    }

    /// `text` with the check icon.
    pub fn check(text: impl Into<String>) -> Self {
        Toast {
            text: text.into(),
            kind: Kind::Check,
        }
    }
}

/// A toast shown, and when, by egui's clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub toast: Toast,
    pub at: f64,
}

impl Shown {
    /// Whether its time is up at `now` (or its time isn't a number).
    fn up(&self, now: f64) -> bool {
        let left = theme::TOAST_SECONDS - (now - self.at);
        left.is_nan() || left <= 0.0
    }
}

/// The toasts shown, oldest first.
#[derive(Debug, Default)]
pub struct Toasts {
    shown: Vec<Shown>,
}

impl Toasts {
    /// `toast` shown from `now`, after (under) those shown already; or, if one shown says
    /// what it says with its icon, that one's clock started again from `now`, and it the
    /// newest, so a key held (each of its repeats asking again) shows one toast.
    pub fn push(&mut self, toast: Toast, now: f64) {
        self.shown
            .retain(|shown| shown.toast.text != toast.text || shown.toast.kind != toast.kind);
        self.shown.push(Shown { toast, at: now });
    }

    /// The toasts shown, oldest first.
    pub fn shown(&self) -> &[Shown] {
        &self.shown
    }

    /// The toasts over `canvas`'s bottom right, by egui's clock: those whose time is up taken
    /// down first; each faded by its age over egui's `animation_time`; the frame they need
    /// asked for ([`next_frame`]).
    pub fn show(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let now = ctx.input(|input| input.time);
        self.shown.retain(|shown| !shown.up(now));
        if self.shown.is_empty() {
            return;
        }
        let fade = f64::from(ctx.global_style().animation_time);
        let corner = canvas.right_bottom()
            - egui::vec2(theme::TOAST_MARGIN_RIGHT, theme::TOAST_MARGIN_BOTTOM);
        egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(corner)
            .interactable(false)
            // Each toast fades by its own age, never the area's.
            .fade_in(false)
            .show(ctx, |ui| {
                // Each frame's right edge the column's, however wide it is
                // (`align-items: flex-end`).
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.y = theme::TOAST_GAP;
                    for shown in &self.shown {
                        ui.scope(|ui| {
                            ui.multiply_opacity(opacity(now - shown.at, fade));
                            one(ui, &shown.toast);
                        });
                    }
                });
            });
        if let Some(wait) = next_frame(&self.shown, now, fade) {
            ctx.request_repaint_after(wait);
        }
    }
}

/// One toast (`.toast`): its icon and its words in a row, each centred across it, 8 apart,
/// in its frame, the words wrapped to what the icon leaves of the toast's width.
fn one(ui: &mut egui::Ui, toast: &Toast) {
    let words = egui::WidgetText::from(RichText::new(&toast.text).color(theme::TOAST_TEXT))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            words_width(),
            egui::TextStyle::Body,
        );
    let text = words.size();
    let row = egui::vec2(
        ICON_SIZE + theme::TOAST_ICON_GAP + text.x,
        text.y.max(ICON_SIZE),
    );
    egui::Frame::NONE
        .fill(theme::TOAST_BG)
        .stroke(egui::Stroke::new(theme::BORDER_WIDTH, theme::TOAST_BORDER))
        .corner_radius(theme::TOAST_CORNER_RADIUS)
        .shadow(theme::TOAST_SHADOW)
        .inner_margin(theme::TOAST_PADDING)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(row, egui::Sense::hover());
            let icon = egui::Rect::from_center_size(
                egui::pos2(rect.left() + ICON_SIZE / 2.0, rect.center().y),
                egui::Vec2::splat(ICON_SIZE),
            );
            let label = egui::Rect::from_min_size(
                egui::pos2(rect.right() - text.x, rect.center().y - text.y / 2.0),
                text,
            );
            ui.put(
                icon,
                toast
                    .kind
                    .icon()
                    .image(IconSize::Small)
                    .tint(theme::TOAST_ICON)
                    .alt_text(toast.kind.alt_text()),
            );
            ui.put(label, egui::Label::new(words));
        });
}

/// The widest a toast's words are: what its icon and the gap after it leave inside the
/// toast's width, which counts its border and padding (`.toast`'s `max-width: 420px` under
/// `box-sizing: border-box`): 420 − 2 − 18 − 22 = 378.
pub fn words_width() -> f32 {
    theme::TOAST_MAX_WIDTH
        - theme::BORDER_WIDTH * 2.0
        - f32::from(theme::TOAST_PADDING.left)
        - f32::from(theme::TOAST_PADDING.right)
        - ICON_SIZE
        - theme::TOAST_ICON_GAP
}

/// A toast's opacity at `age` seconds since it was shown: whole until its last `fade`
/// seconds, falling to 0 over them; whole with no fade.
pub fn opacity(age: f64, fade: f64) -> f32 {
    if fade.is_nan() || fade <= 0.0 {
        return 1.0;
    }
    let left = theme::TOAST_SECONDS - age;
    let opacity = (left / fade).clamp(0.0, 1.0);
    if opacity.is_nan() {
        1.0
    } else {
        opacity as f32
    }
}

/// How long after `now` the next frame is needed for `shown`: none with none shown; else
/// when the first to fade begins to (its time left less `fade`), which is at once while any
/// fades, a wait of none or less; with no fade, when the first is taken down. A `fade` that
/// isn't a number, or is less than none, is none (`f64::max` gives 0 for either).
pub fn next_frame(shown: &[Shown], now: f64, fade: f64) -> Option<Duration> {
    let fade = fade.max(0.0);
    shown
        .iter()
        .map(|shown| theme::TOAST_SECONDS - (now - shown.at) - fade)
        .min_by(f64::total_cmp)
        .map(|wait| Duration::try_from_secs_f64(wait).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FADE: f64 = 0.2;

    fn shown(at: f64) -> Shown {
        Shown {
            toast: Toast::info("x"),
            at,
        }
    }

    #[test]
    fn the_constructors_give_the_prototypes_icons() {
        // `toast(text)` and `toast(text, 'check')`.
        assert_eq!(
            Toast::info("a"),
            Toast {
                text: "a".to_owned(),
                kind: Kind::Info,
            }
        );
        assert_eq!(
            Toast::check("b"),
            Toast {
                text: "b".to_owned(),
                kind: Kind::Check,
            }
        );
        assert_eq!(
            [Kind::Info, Kind::Check].map(Kind::icon),
            [Icon::Info, Icon::Check]
        );
    }

    #[test]
    fn a_toast_saying_what_one_shown_says_with_its_icon_starts_its_clock_again_unstacked() {
        let mut toasts = Toasts::default();
        toasts.push(Toast::info("a"), 0.0);
        toasts.push(Toast::info("b"), 1.0);
        // "a" again with its icon: not stacked, its clock from 2, and the newest.
        toasts.push(Toast::info("a"), 2.0);
        let said = |toasts: &Toasts| -> Vec<(String, Kind, f64)> {
            toasts
                .shown()
                .iter()
                .map(|shown| (shown.toast.text.clone(), shown.toast.kind, shown.at))
                .collect()
        };
        assert_eq!(
            said(&toasts),
            [
                ("b".to_owned(), Kind::Info, 1.0),
                ("a".to_owned(), Kind::Info, 2.0)
            ]
        );
        // The same words with another icon are another toast.
        toasts.push(Toast::check("a"), 3.0);
        assert_eq!(said(&toasts).len(), 3);
        // Thirty more of one: still one of it.
        for at in 0..30 {
            toasts.push(Toast::check("a"), 4.0 + f64::from(at) / 30.0);
        }
        assert_eq!(said(&toasts).len(), 3);
    }

    #[test]
    fn opacity_is_whole_at_once_and_falls_over_the_last_of_its_three_point_eight_seconds() {
        // By hand: a 0.2 s fade out at the end of a 3.8 s life.
        assert_eq!(opacity(0.0, FADE), 1.0);
        assert_eq!(opacity(0.05, FADE), 1.0);
        assert_eq!(opacity(1.9, FADE), 1.0);
        // 0.05 s left: a quarter.
        assert!((opacity(3.75, FADE) - 0.25).abs() < 1e-6);
        assert_eq!(opacity(3.6, FADE), 1.0);
        // 0.1 s left of 3.8: half.
        assert!((opacity(3.7, FADE) - 0.5).abs() < 1e-6);
        assert_eq!(opacity(3.8, FADE), 0.0);
        assert_eq!(opacity(5.0, FADE), 0.0);
        // No fade: whole while shown, and past its time too (as a fade of none or less, or
        // one that isn't a number, gives no fade at all, at any age).
        assert_eq!(opacity(0.0, 0.0), 1.0);
        assert_eq!(opacity(5.0, 0.0), 1.0);
        assert_eq!(opacity(1.0, -0.2), 1.0);
        assert_eq!(opacity(5.0, f64::NAN), 1.0);
        assert_eq!(opacity(f64::NAN, FADE), 1.0);
    }

    #[test]
    fn a_frame_is_asked_at_once_only_while_one_fades_and_none_with_none_shown() {
        assert_eq!(next_frame(&[], 10.0, FADE), None);
        // Just shown, whole: its fade begins at 13.6, 3.6 s on.
        let wait = next_frame(&[shown(10.0)], 10.0, FADE).unwrap();
        assert!((wait.as_secs_f64() - 3.6).abs() < 1e-9, "{wait:?}");
        // Whole, shown at 10: its fade out begins at 13.6, 2.6 s after 11.
        let wait = next_frame(&[shown(10.0)], 11.0, FADE).unwrap();
        assert!((wait.as_secs_f64() - 2.6).abs() < 1e-9, "{wait:?}");
        // Two whole: the older's fade comes first (13.6 - 12 = 1.6).
        let wait = next_frame(&[shown(10.0), shown(11.0)], 12.0, FADE).unwrap();
        assert!((wait.as_secs_f64() - 1.6).abs() < 1e-9, "{wait:?}");
        // One fading out among them: at once.
        assert_eq!(
            next_frame(&[shown(10.0), shown(11.0)], 13.7, FADE),
            Some(Duration::ZERO)
        );
        // No fade: the frame that takes it down, at 13.8.
        let wait = next_frame(&[shown(10.0)], 11.0, 0.0).unwrap();
        assert!((wait.as_secs_f64() - 2.8).abs() < 1e-9, "{wait:?}");
        // At the fade's start exactly (13.6), and just before it: at once, and 0.1 s on.
        assert_eq!(next_frame(&[shown(10.0)], 13.6, FADE), Some(Duration::ZERO));
        let wait = next_frame(&[shown(10.0)], 13.5, FADE).unwrap();
        assert!((wait.as_secs_f64() - 0.1).abs() < 1e-9, "{wait:?}");
        // Past its time (not yet taken down): at once.
        assert_eq!(next_frame(&[shown(10.0)], 14.0, FADE), Some(Duration::ZERO));
        // A fade less than none, or not a number, is none: 2.8 s on from 11.
        for fade in [-0.2, f64::NAN] {
            let wait = next_frame(&[shown(10.0)], 11.0, fade).unwrap();
            assert!((wait.as_secs_f64() - 2.8).abs() < 1e-9, "{fade}: {wait:?}");
        }
    }

    /// The fills and borders of the toasts' frames painted, top to bottom: each frame a
    /// rectangle filled Gray.200 (`#212121`, `.toast`'s `var(--g200)`), its shadow aside.
    fn frames<S>(
        h: &egui_kittest::Harness<'_, S>,
    ) -> Vec<(egui::Rect, egui::Color32, egui::Color32)> {
        fn flatten(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
                shape => out.push(shape.clone()),
            }
        }
        let mut shapes = Vec::new();
        for clipped in &h.output().shapes {
            flatten(&clipped.shape, &mut shapes);
        }
        let mut frames: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match shape {
                // Gray.200 under whatever opacity, its colour read back unmultiplied.
                egui::Shape::Rect(rect)
                    if rect.blur_width == 0.0
                        && rect.stroke.width > 0.0
                        && rect.fill.a() > 0
                        && rect.fill.to_srgba_unmultiplied()[..3]
                            .iter()
                            .all(|channel| channel.abs_diff(0x21) <= 3) =>
                {
                    Some((rect.rect, rect.fill, rect.stroke.color))
                }
                _ => None,
            })
            .collect();
        frames.sort_by(|a, b| a.0.top().total_cmp(&b.0.top()));
        frames
    }

    /// Every rectangle's fill and blur painted: what a test failing says it found.
    fn rect_fills<S>(h: &egui_kittest::Harness<'_, S>) -> Vec<String> {
        fn flatten(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
                egui::Shape::Rect(rect) => out.push(format!("{:?} {}", rect.fill, rect.blur_width)),
                other => out.push(format!("{other:?}").chars().take(40).collect()),
            }
        }
        let mut out = Vec::new();
        for clipped in &h.output().shapes {
            flatten(&clipped.shape, &mut out);
        }
        out
    }

    /// A harness drawing toasts over an 800 by 600 canvas, `toasts` shown from its clock's
    /// time now (egui's: the harness steps it as it's built), in order.
    fn harness(toasts: Vec<Toast>) -> egui_kittest::Harness<'static, Toasts> {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(800.0, 600.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                move |ui, toasts: &mut Toasts| toasts.show(ui.ctx(), canvas),
                Toasts::default(),
            );
        theme::apply(&h.ctx);
        let now = h.ctx.input(|input| input.time);
        for toast in toasts {
            h.state_mut().push(toast, now);
        }
        h
    }

    #[test]
    fn toasts_stack_newest_last_each_with_its_icon_bordered_gray() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let mut h = harness(vec![
            Toast::info("one"),
            Toast::check("two"),
            Toast::info("three"),
        ]);
        h.step();
        h.step();
        // Each icon by its name, 14 square, 8 left of its words (`.toast`'s `gap: 8px`).
        let mut tops = Vec::new();
        for (text, alt) in [("one", "Info"), ("two", "Done"), ("three", "Info")] {
            let words = h.get_by_label(text).rect();
            let icon = h
                .query_all_by_role(egui::accesskit::Role::Image)
                .find(|node| (node.rect().center().y - words.center().y).abs() <= 1.0)
                .unwrap_or_else(|| panic!("no icon beside {text}"));
            assert_eq!(
                icon.accesskit_node().label().as_deref(),
                Some(alt),
                "{text}"
            );
            assert!(
                (words.left() - icon.rect().right() - 8.0).abs() <= 0.5,
                "{text}"
            );
            assert!((icon.rect().width() - 14.0).abs() <= 0.5, "{text}");
            tops.push(words.top());
        }
        // The newest last: under the others.
        assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
        // Three frames, 6 apart, their right edges in line (`align-items: flex-end`), each
        // bordered Gray.250 (`#272626`).
        let frames = frames(&h);
        assert_eq!(frames.len(), 3, "{frames:?} of {:?}", rect_fills(&h));
        for pair in frames.windows(2) {
            assert!(
                (pair[1].0.top() - pair[0].0.bottom() - 6.0).abs() <= 0.5,
                "{frames:?}"
            );
            assert!(
                (pair[1].0.right() - pair[0].0.right()).abs() <= 0.5,
                "{frames:?}"
            );
        }
        assert_eq!(
            frames.iter().map(|frame| frame.2).collect::<Vec<_>>(),
            [
                egui::Color32::from_rgb(0x27, 0x26, 0x26),
                egui::Color32::from_rgb(0x27, 0x26, 0x26),
                egui::Color32::from_rgb(0x27, 0x26, 0x26),
            ]
        );
    }

    #[test]
    fn a_long_toasts_words_wrap_to_378_so_the_toast_is_at_most_420_wide() {
        use egui_kittest::kittest::Queryable;
        // `.toast`'s `max-width: 420px`, border-box (styles.css): 420 less its border (2),
        // its padding (10 + 8), and its icon and gap (14 + 8) leaves its words 378.
        assert_eq!(words_width(), 378.0);
        let long = "word ".repeat(60);
        let long = long.trim_end();
        let mut h = harness(vec![Toast::info(long)]);
        h.step();
        h.step();
        let words = h.get_by_label(long).rect();
        let frames = frames(&h);
        assert_eq!(frames.len(), 1, "{frames:?}");
        let frame = frames[0].0;
        // Wrapped onto several lines, each at most 378 wide, filling them nearly (a word and
        // its space are under 40), and the frame round them at most 420.
        assert!(words.height() > 40.0, "{words:?}");
        assert!(words.width() <= 378.0 + 0.5, "{words:?}");
        assert!(words.width() > 378.0 - 40.0, "{words:?}");
        assert!(frame.width() <= 420.0 + 0.5, "{frame:?}");
        assert!(
            (frame.width() - (words.width() + 2.0 + 18.0 + 22.0)).abs() <= 0.5,
            "{frame:?} {words:?}"
        );
    }

    #[test]
    fn a_toast_shows_whole_at_once_and_fades_out_over_egui_animation_time() {
        // Whole from its first frame seen, then faded over the last of its time by the
        // animation time, 0.2 s here: its fill's alpha, out of 255, the opacity by hand.
        let mut h = harness(vec![Toast::info("one")]);
        h.ctx.all_styles_mut(|style| style.animation_time = 0.2);
        let shown = h.state().shown()[0].at;
        let age = |h: &egui_kittest::Harness<'_, Toasts>| h.ctx.input(|input| input.time) - shown;
        let alpha_at = |h: &mut egui_kittest::Harness<'_, Toasts>| {
            h.step();
            let frames = frames(h);
            (age(h), frames.first().map(|frame| frame.1.a()))
        };
        // The area's first frame ever sizes it, unseen (egui's sizing pass); the next shows
        // it whole.
        h.step();
        let (now, alpha) = alpha_at(&mut h);
        assert_eq!(alpha, Some(255), "at {now}");
        // Once its icon has loaded (whose first frames egui repaints), it asks for the frame
        // its fade begins at (3.6 s), egui taking a frame off what's asked.
        while repaint_delay(&h) == Duration::ZERO {
            assert!(age(&h) < 0.5, "a frame asked for at once at {}", age(&h));
            h.step();
        }
        let now = age(&h);
        let wait = repaint_delay(&h).as_secs_f64();
        assert!(
            (wait - (3.6 - now)).abs() <= 1.0 / 60.0 + 1e-6,
            "{wait} at {now}"
        );
        while age(&h) < 1.0 {
            h.step();
        }
        let (now, alpha) = alpha_at(&mut h);
        assert_eq!(alpha, Some(255), "at {now}");
        assert_ne!(repaint_delay(&h), Duration::ZERO);
        // Out: (left / 0.2) of 255, each frame asked for at once.
        while age(&h) < 3.65 {
            h.step();
        }
        for _ in 0..3 {
            let (now, alpha) = alpha_at(&mut h);
            let want = 255.0 * ((3.8 - now) / 0.2);
            assert!(
                alpha.is_some_and(|alpha| (f64::from(alpha) - want).abs() <= 1.5),
                "{alpha:?} at {now}, want {want}"
            );
            assert_eq!(repaint_delay(&h), Duration::ZERO);
        }
        // Gone at 3.8, and no frame asked for after.
        while age(&h) < 3.8 {
            h.step();
        }
        h.step();
        assert!(h.state().shown().is_empty());
        assert_eq!(frames(&h), []);
        h.step();
        assert_eq!(repaint_delay(&h), Duration::MAX);
    }

    fn repaint_delay<S>(h: &egui_kittest::Harness<'_, S>) -> Duration {
        h.output()
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |viewport| viewport.repaint_delay)
    }

    #[test]
    fn each_toast_is_taken_down_on_its_own_clock() {
        let ctx = egui::Context::default();
        let mut toasts = Toasts::default();
        toasts.push(Toast::info("first"), 0.0);
        toasts.push(Toast::check("second"), 1.0);
        let texts = |toasts: &Toasts| -> Vec<String> {
            toasts
                .shown()
                .iter()
                .map(|shown| shown.toast.text.clone())
                .collect()
        };
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        for (now, want) in [
            (3.79, vec!["first", "second"]),
            (3.8, vec!["second"]),
            (4.79, vec!["second"]),
            (4.8, vec![]),
        ] {
            let mut input = egui::RawInput {
                time: Some(now),
                ..Default::default()
            };
            input.screen_rect = Some(canvas);
            let mut output = ctx.run_ui(input, |ui| toasts.show(ui.ctx(), canvas));
            output.textures_delta.clear();
            assert_eq!(texts(&toasts), want, "at {now}");
        }
    }
}
