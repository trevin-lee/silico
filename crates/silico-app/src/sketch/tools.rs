//! Sketch mode's tools (M5.11b and c; docs/design/m5.0-app.md, section 9, *Sketching*): the
//! drawing tools, each the core's [`SketchTool`] (#335) driven through its API alone, with
//! no rule of its own copied here; Smart Dimension, the core's [`SmartDimension`] (#345),
//! likewise; trim and split, the core's `sketch_trim`, `sketch_split`, and `sketch_extend`
//! asked from the pointer's picks; and Extend, SolidWorks' Extend Entities, the core's hover
//! (`extend_end`, `extend_preview`) on the curve trim and split would pick, and a click's
//! `sketch_extend` to the next curve. What a tool asks of the core comes out as
//! [`Request`]s, which the tab sends through its queue, never waited on; the core's answer
//! to an add is given back by its token ([`Tools::added`], [`Tools::refused`]), in whatever
//! order it comes.
//!
//! What's here of trim and split is only where the pointer is: which curve it's on (within
//! the pick tolerance, as sketch mode's selection hits one), where a drag's path crosses a
//! curve, and, for a split, the click put on its curve (the core refuses a position farther
//! than 1e-7 mm from it, decision 4). Where to cut, what's removed, and what's kept are the
//! core's.

use std::collections::BTreeSet;
use std::f64::consts::{PI, TAU};

use silico_core::command::sketch::SketchRead;
use silico_core::command::{ExtendBy, Revision};
use silico_core::document::sketch_edit::Members;
use silico_core::document::{Coordinate, ElementId, ElementKind, FeatureId};
use silico_core::extend::End;
use silico_core::sketch_tools::{
    AddToken, Axes, Context, DimensionEvent, DimensionPreview, Event, Modifiers, NewIds, Offer,
    Outcome, Preview, SketchTool, SmartDimension, SystemIds, ToolKind, extend_end, extend_preview,
};

use super::draw::{
    Curve, Geometry, add, angle, direction, dot, foot, length, mul, segment_distance, sub,
};

/// The tool chosen: a drawing tool of the core's, Smart Dimension, trim, split, or extend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Draw(ToolKind),
    Dimension,
    Trim,
    Split,
    Extend,
}

/// How trim trims: SolidWorks' Power trim, a drag across curves trimming each it crosses and
/// a drag along one extending it; or Trim to closest, a click removing the curve up to its
/// closest cuts and a drag from one extending it to the next curve (docs/ux/solidworks.md,
/// §4, *Trim, extend, split*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrimMode {
    #[default]
    Power,
    Closest,
}

/// What a tool asks of the core, in the order it asks: a drawing tool's or Smart
/// Dimension's `sketch_add`, with its token, and trim's, split's, and extend's edits, each
/// of a curve of the newest read, an extend's with the tool it's `from`.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Add {
        token: AddToken,
        members: Members,
    },
    Trim {
        curve: ElementId,
        pick: [f64; 2],
    },
    Split {
        curve: ElementId,
        at: Vec<[f64; 2]>,
    },
    Extend {
        curve: ElementId,
        end: End,
        by: ExtendBy,
        from: ExtendFrom,
    },
}

/// Which tool asked an extend, which its undo step is named by: Trim's drag, a sketch edit
/// as any other ("Edit" and the sketch), or Extend's click ("Extend", as its button says).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendFrom {
    Trim,
    Extend,
}

/// What a tool works against for one event: the newest read, of the sketch `sketch`, of the
/// model at `revision`; the pick tolerance there, in millimetres, and which axes the pointer
/// may take there (the core's [`Axes`], as sketch mode's selection works them out for the
/// same pointer); and the palette's settings it takes.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub read: &'a SketchRead,
    pub sketch: &'a FeatureId,
    pub revision: Revision,
    pub tolerance: f64,
    pub axes: Axes,
    pub automatic_relations: bool,
    pub construction: bool,
    pub trim: TrimMode,
}

impl Input<'_> {
    /// The core's context for a drawing tool.
    fn context(&self) -> Context<'_> {
        Context {
            sketch_id: self.sketch,
            revision: self.revision,
            sketch: &self.read.members,
            tolerance: self.tolerance,
            automatic_relations: self.automatic_relations,
            construction: self.construction,
            states: &self.read.states,
            axes: self.axes,
        }
    }

    /// The tolerance, as the core takes one: none that isn't a positive number.
    fn tolerance(&self) -> f64 {
        if self.tolerance.is_finite() && self.tolerance > 0.0 {
            self.tolerance
        } else {
            0.0
        }
    }
}

/// The tool on, between frames.
enum Active {
    Draw(Box<SketchTool>),
    Dimension(Box<SmartDimension>),
    Trim(Trim),
    Split(Split),
    Extend(Extend),
}

/// Trim, between events: a press under way, with what it was on (a curve, its kind, and its
/// geometry then), where it was, whether it has left the pick tolerance of there, whether
/// every place since has stayed within it of that curve's line or round, and, for a round,
/// the signed angle the path has turned about its centre while it did (counter-clockwise
/// positive, each step's change taken the short way round); the path's last place and its
/// trail (the last [`TRAIL`] places, for drawing), and the curves the path
/// has crossed so far, each with where it first crossed it.
#[derive(Debug, Default)]
enum Trim {
    #[default]
    Idle,
    Pressed {
        on: Option<(ElementId, ElementKind, Curve)>,
        press: [f64; 2],
        dragged: bool,
        along: bool,
        turned: f64,
        last: [f64; 2],
        trail: Vec<[f64; 2]>,
        crossed: Vec<(ElementId, [f64; 2])>,
    },
}

/// How many of a power trim's places its trail draws: the newest.
pub const TRAIL: usize = 256;

/// Split, between events: a press under way, and a circle's first place, the click that a
/// second on the same circle splits it with.
#[derive(Debug, Default)]
struct Split {
    pressed: Option<[f64; 2]>,
    first: Option<(ElementId, [f64; 2])>,
}

/// Extend, between events: a press under way, where it was; and, once a click has asked an
/// extend, the revision of the read it was planned on. Until a newer read (or the extend's
/// refusal) lets it go, a click asks nothing: on that read, the curve it extended is drawn
/// short still, so a second click would plan its end again, past the curve it now reaches.
#[derive(Debug, Default)]
struct Extend {
    pressed: Option<[f64; 2]>,
    sent: Option<Revision>,
}

/// Sketch mode's tools: the one on, if one is; where new members' IDs come from; what the
/// tool says and draws; and what it asks of the core, until the tab takes it.
pub struct Tools {
    active: Option<Active>,
    /// Where a drawing tool's new members' IDs come from: the system's randomness, as the
    /// core draws them (a test gives its own).
    pub ids: Box<dyn NewIds>,
    said: Option<String>,
    preview: Preview,
    requests: Vec<Request>,
    /// Where the pointer was last given to the tool: a hover is given once it moves.
    last: Option<[f64; 2]>,
    /// Smart Dimension's Modify box: how many times one has opened (what a box's focus is
    /// keyed on, so each new one takes the keyboard once), and where on the plane the click
    /// that opened the one open was, where its dimension's text stays while it's open.
    pub modify_opened: u64,
    modify_at: Option<[f64; 2]>,
}

impl Default for Tools {
    fn default() -> Self {
        Tools {
            active: None,
            ids: Box::new(SystemIds::new()),
            said: None,
            preview: Preview::default(),
            requests: Vec::new(),
            last: None,
            modify_opened: 0,
            modify_at: None,
        }
    }
}

/// What a tool says when it's asked to work where it can't.
pub const NOTHING_TO_TRIM: &str =
    "nothing to trim there: click a line, an arc, or a circle, or drag across them";
pub const NOTHING_TO_SPLIT: &str = "nothing to split there: click a line, an arc, or a circle";
pub const SECOND_ON_CIRCLE: &str =
    "a circle splits at two places: click a second place on the same circle";
pub const NO_END_TO_EXTEND: &str = "a circle has no end to extend";
pub const NOTHING_TO_EXTEND: &str = "nothing to extend there: click a line or an arc";

impl Tools {
    /// The tool chosen, if one is.
    pub fn choice(&self) -> Option<Choice> {
        self.active.as_ref().map(|active| match active {
            Active::Draw(tool) => Choice::Draw(tool.kind()),
            Active::Dimension(_) => Choice::Dimension,
            Active::Trim(_) => Choice::Trim,
            Active::Split(_) => Choice::Split,
            Active::Extend(_) => Choice::Extend,
        })
    }

    /// `choice` chosen: another than the one on starts afresh, ending the gesture under way
    /// with nothing added (section 9, *Line*: choosing another tool ends a chain); the one
    /// on goes on as it was; none leaves it.
    pub fn choose(&mut self, choice: Option<Choice>) {
        if choice == self.choice() {
            return;
        }
        self.active = choice.map(|choice| match choice {
            Choice::Draw(kind) => Active::Draw(Box::new(SketchTool::new(kind))),
            Choice::Dimension => Active::Dimension(Box::new(SmartDimension::new())),
            Choice::Trim => Active::Trim(Trim::Idle),
            Choice::Split => Active::Split(Split::default()),
            Choice::Extend => Active::Extend(Extend::default()),
        });
        self.preview = Preview::default();
        self.said = None;
        self.last = None;
        self.modify_at = None;
    }

    /// Whether a gesture is under way: a press held, a chain or an entity begun, or a
    /// circle's first split place clicked.
    pub fn busy(&self) -> bool {
        match &self.active {
            None => false,
            Some(Active::Draw(tool)) => tool.busy(),
            Some(Active::Dimension(tool)) => tool.busy(),
            Some(Active::Trim(trim)) => matches!(trim, Trim::Pressed { .. }),
            Some(Active::Split(split)) => split.pressed.is_some() || split.first.is_some(),
            Some(Active::Extend(extend)) => extend.pressed.is_some(),
        }
    }

    /// What the tool draws now.
    pub fn preview(&self) -> &Preview {
        &self.preview
    }

    /// What the tool last said, until it adds something or another tool is chosen.
    pub fn said(&self) -> Option<&str> {
        self.said.as_deref()
    }

    /// What it has asked of the core since this was last taken, in order.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// The core took the add `token` names, at `revision`.
    pub fn added(&mut self, token: AddToken, revision: Revision) {
        match &mut self.active {
            Some(Active::Draw(tool)) => tool.added(token, revision),
            Some(Active::Dimension(tool)) => tool.added(token, revision),
            _ => {}
        }
    }

    /// The core refused the add `token` names: the tool forgets it.
    pub fn refused(&mut self, token: AddToken) {
        match &mut self.active {
            Some(Active::Draw(tool)) => tool.refused(token),
            Some(Active::Dimension(tool)) => tool.refused(token),
            _ => {}
        }
    }

    /// What Smart Dimension draws now, if it's the tool on.
    pub fn dimension_preview(&self) -> Option<&DimensionPreview> {
        match &self.active {
            Some(Active::Dimension(tool)) => Some(tool.preview()),
            _ => None,
        }
    }

    /// Where on the plane the Modify box open was opened: where its dimension's text
    /// stays while it's open.
    pub fn modify_at(&self) -> Option<[f64; 2]> {
        self.modify_at
            .filter(|_| self.dimension_preview().is_some_and(|p| p.modify.is_some()))
    }

    /// The offer Smart Dimension holds open for the last dimension it placed (SD7), if it's
    /// the tool on and holds one.
    pub fn open_offer(&self) -> Option<&Offer> {
        match &self.active {
            Some(Active::Dimension(tool)) => tool.open_offer(),
            _ => None,
        }
    }

    /// The core refused the extend Extend's click asked: it may click again, on the read it
    /// has.
    pub fn extend_refused(&mut self) {
        if let Some(Active::Extend(extend)) = &mut self.active {
            extend.sent = None;
        }
    }

    /// Smart Dimension's offer answered (its request sent, or Leave): let go, so the next
    /// dimension may be placed.
    pub fn dismiss_offer(&mut self) {
        if let Some(Active::Dimension(tool)) = &mut self.active {
            tool.dismiss_offer();
        }
    }

    /// A newer read, given to Smart Dimension if it's on (`SmartDimension::settle`): its
    /// offer settled or released, its picks and its Modify box measured again; what that
    /// says kept. Extend lets its click go once the read is newer than the one it was planned
    /// on; and, with no press under way, hovers again where the pointer was last, so its
    /// preview and what it says are of the newest read (a click's extend shown done).
    pub fn settle(&mut self, input: &Input<'_>) {
        if let Some(Active::Extend(extend)) = &mut self.active {
            if extend.sent.is_some_and(|sent| input.revision > sent) {
                extend.sent = None;
            }
            if extend.pressed.is_none()
                && let Some(at) = self.last
            {
                (self.preview, self.said) = extend_hover(input, at);
            }
            return;
        }
        let Some(Active::Dimension(tool)) = &mut self.active else {
            return;
        };
        let outcome = tool.settle(&input.context());
        self.dimension_took(outcome);
    }

    /// `event` given to Smart Dimension, if it's the tool on, against `input`: what it adds
    /// queued as a request, what it says kept, and whether the Modify box opened noted.
    pub fn dimension(&mut self, input: &Input<'_>, event: DimensionEvent) {
        if let DimensionEvent::Press(at)
        | DimensionEvent::Move(at)
        | DimensionEvent::Release(at)
        | DimensionEvent::RightClick(at) = event
        {
            self.last = Some(at);
        }
        let Some(Active::Dimension(tool)) = &mut self.active else {
            return;
        };
        // What was said of the text typed before is let go with it.
        if matches!(event, DimensionEvent::Typed(_)) {
            self.said = None;
        }
        let outcome = tool.handle(&input.context(), self.ids.as_mut(), event);
        self.dimension_took(outcome);
    }

    /// What Smart Dimension's step did: the Modify box's opening noted (where the click that
    /// opened it was), and the outcome taken.
    fn dimension_took(&mut self, outcome: Outcome) {
        let Some(Active::Dimension(tool)) = &self.active else {
            return;
        };
        let preview = tool.preview();
        match (&preview.modify, self.modify_at) {
            (Some(_), None) => {
                self.modify_opened += 1;
                self.modify_at = preview.pointer;
            }
            (None, Some(_)) => self.modify_at = None,
            _ => {}
        }
        self.took(outcome);
    }

    /// A tool's outcome: what it said kept; what it added queued as a request (a drawing
    /// tool's curves marked as construction if `construction`), letting go of what it said;
    /// and the tool left if it was.
    fn took(&mut self, outcome: Outcome) {
        if let Some(message) = &outcome.message {
            self.said = Some(message.text());
        }
        if let (Some(members), Some(token)) = (outcome.add, outcome.token) {
            self.said = None;
            self.requests.push(Request::Add { token, members });
        }
        if outcome.left {
            self.choose(None);
        }
    }

    /// Everything the tool added and no read has shown forgotten, with the gesture under
    /// way: for a sketch mode whose session went back to the file (a reload).
    pub fn forget_pending(&mut self) {
        if let Some(Active::Draw(tool)) = &mut self.active {
            tool.forget_pending();
        }
        if let Some(Active::Dimension(tool)) = &mut self.active {
            tool.forget_pending();
            self.modify_at = None;
        }
        if let Some(Active::Trim(trim)) = &mut self.active {
            *trim = Trim::Idle;
        }
        if let Some(Active::Split(split)) = &mut self.active {
            *split = Split::default();
        }
        if let Some(Active::Extend(extend)) = &mut self.active {
            *extend = Extend::default();
        }
        self.preview = Preview::default();
    }

    /// Esc, with a tool on (section 9, *Line*: Esc ends a chain and leaves the tool): the
    /// gesture ended and the tool left. A drawing tool is told, with `input` if there's a
    /// read to give it. Smart Dimension takes it as its own (SD2 and section 9: with the
    /// Modify box open it places the measured value; else it undoes the last pick, and with
    /// none leaves the tool). Whether there was a tool to take it.
    pub fn escape(&mut self, input: Option<&Input<'_>>) -> bool {
        if let (Some(Active::Dimension(_)), Some(input)) = (&self.active, input) {
            self.dimension(input, DimensionEvent::Escape);
            return true;
        }
        let Some(active) = &mut self.active else {
            return false;
        };
        if let (Active::Draw(tool), Some(input)) = (active, input) {
            tool.handle(
                &input.context(),
                self.ids.as_mut(),
                Event::Escape,
                Modifiers::default(),
            );
        }
        self.choose(None);
        true
    }

    /// A press ended with no release (focus lost, a navigation tool taking the drag, the tab
    /// hidden), or a release off the plane or with no read to give it: the press dropped
    /// as if never made, the tool kept, so no press outlives its release. With `input`, the
    /// tool is sent [`Event::Cancel`], which puts its gesture back as it was before the
    /// press (a chain's placed points, and the points the pointer woke, kept); with no read
    /// to give it, the tool is started afresh ([`SketchTool::forget_pending`], which
    /// forgets its unanswered adds too: a click finds their points again only once a read
    /// holds them, so until then one at their place makes a point of its own).
    pub fn cancel(&mut self, input: Option<&Input<'_>>) {
        if let Some(input) = input {
            self.handle(input, Event::Cancel, Modifiers::default());
            return;
        }
        match &mut self.active {
            Some(Active::Draw(tool)) => tool.forget_pending(),
            // With no read to give it, nothing: its press is a flag, which the next press
            // sets again, so the cancelled one never clicks, and its picks stay.
            Some(Active::Dimension(_)) => {}
            Some(Active::Trim(trim)) => *trim = Trim::Idle,
            Some(Active::Split(split)) => *split = Split::default(),
            // The press dropped; a click's extend, still unread, kept.
            Some(Active::Extend(extend)) => extend.pressed = None,
            None => {}
        }
        self.preview = Preview::default();
        self.last = None;
    }

    /// The place on the plane the tool was last given.
    pub fn last(&self) -> Option<[f64; 2]> {
        self.last
    }

    /// `event`, with `keys` held, given to the tool on against `input`: what it adds or
    /// edits queued as requests, what it says kept, and what it draws.
    pub fn handle(&mut self, input: &Input<'_>, event: Event, keys: Modifiers) {
        if let Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) =
            event
        {
            self.last = Some(at);
        }
        match &mut self.active {
            None => {}
            Some(Active::Draw(tool)) => {
                let outcome = tool.handle(&input.context(), self.ids.as_mut(), event, keys);
                self.preview = tool.preview().clone();
                self.took(outcome);
            }
            Some(Active::Dimension(_)) => {
                let event = match event {
                    Event::Press(at) => DimensionEvent::Press(at),
                    Event::Move(at) => DimensionEvent::Move(at),
                    Event::Release(at) => DimensionEvent::Release(at),
                    Event::Enter | Event::EndChain => DimensionEvent::Enter,
                    Event::Escape => DimensionEvent::Escape,
                    // A press ended with no release: dropped, its picks kept (#365).
                    Event::Cancel => DimensionEvent::Cancel,
                    // Its second click's press and release were its own.
                    Event::DoubleClick(_) => return,
                };
                self.dimension(input, event);
            }
            Some(Active::Trim(trim)) => {
                let asked = self.requests.len();
                let said = trim_event(trim, input, event, &mut self.requests, &mut self.preview);
                self.say(said, asked);
            }
            Some(Active::Split(split)) => {
                let asked = self.requests.len();
                let said = split_event(split, input, event, &mut self.requests, &mut self.preview);
                self.say(said, asked);
            }
            Some(Active::Extend(extend)) => {
                if let Some(said) =
                    extend_event(extend, input, event, &mut self.requests, &mut self.preview)
                {
                    self.said = said;
                }
            }
        }
    }
}

impl Tools {
    /// What trim or split said kept, if it said anything; or, if it asked for an edit since
    /// it had asked `asked`, what it said before let go.
    fn say(&mut self, said: Option<&str>, asked: usize) {
        if let Some(said) = said {
            self.said = Some(said.to_owned());
        } else if self.requests.len() > asked {
            self.said = None;
        }
    }
}

/// The curve of `read` nearest `at`, within `tolerance` millimetres: its ID, its kind, and
/// its geometry; ties to the lower ID. Points don't stand in the way, as they do for a
/// selection: trim and split act on curves alone.
pub(crate) fn curve_at(
    read: &SketchRead,
    at: [f64; 2],
    tolerance: f64,
) -> Option<(ElementId, ElementKind, Curve)> {
    if !(at[0].is_finite() && at[1].is_finite()) || tolerance.is_nan() || tolerance < 0.0 {
        return None;
    }
    Geometry::of(read)
        .curves()
        .map(|(id, curve, kind)| (distance(curve, at), id, kind, curve))
        .filter(|(d, ..)| *d <= tolerance)
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, id, kind, curve)| (id.clone(), kind, curve))
}

/// How far `at` is from `curve`, as trim and split take a curve under the pointer: from a
/// line's segment, a circle's rim, or an arc's (or its nearer end, past its sweep).
fn distance(curve: Curve, at: [f64; 2]) -> f64 {
    match curve {
        Curve::Line { from, to } => segment_distance(at, from, to),
        Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } => {
            let off = sub(at, centre);
            let within = (angle(off) - start).rem_euclid(TAU) <= sweep;
            if sweep >= TAU || within {
                (length(off) - radius).abs()
            } else {
                let end = |a: f64| add(centre, mul(direction(a), radius));
                length(sub(at, end(start))).min(length(sub(at, end(start + sweep))))
            }
        }
    }
}

/// `at` put on `curve`: the nearest place of a line's line, or of an arc's or a circle's
/// round; none at a round's centre, which is as near every place of it.
pub(crate) fn onto(curve: Curve, at: [f64; 2]) -> Option<[f64; 2]> {
    let on = match curve {
        Curve::Line { from, to } => foot(at, from, to),
        Curve::Round { centre, radius, .. } => {
            let off = sub(at, centre);
            let l = length(off);
            if !(l > 0.0 && l.is_finite()) {
                return None;
            }
            add(centre, mul(off, radius / l))
        }
    };
    (on[0].is_finite() && on[1].is_finite()).then_some(on)
}

/// Where the segment from `p` to `q` crosses `curve`, by the share of the way along it
/// (0 at `p`, 1 at `q`), each with the place: a line's segment, a circle's rim, or an arc's
/// within its sweep.
pub(crate) fn crossings(p: [f64; 2], q: [f64; 2], curve: Curve) -> Vec<(f64, [f64; 2])> {
    let d = sub(q, p);
    let within = |t: f64| (0.0..=1.0).contains(&t);
    match curve {
        Curve::Line { from, to } => {
            let e = sub(to, from);
            let denominator = cross(d, e);
            if denominator == 0.0 || !denominator.is_finite() {
                return Vec::new();
            }
            let w = sub(from, p);
            let t = cross(w, e) / denominator;
            let u = cross(w, d) / denominator;
            if within(t) && within(u) {
                vec![(t, add(p, mul(d, t)))]
            } else {
                Vec::new()
            }
        }
        Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } => {
            // |p + d t − centre|² = radius²: a t² + 2 b t + c = 0.
            let f = sub(p, centre);
            let a = dot(d, d);
            let b = dot(f, d);
            let c = dot(f, f) - radius * radius;
            let discriminant = b * b - a * c;
            if a <= 0.0 || !discriminant.is_finite() || discriminant < 0.0 {
                return Vec::new();
            }
            let root = discriminant.sqrt();
            let mut found = Vec::new();
            for t in [(-b - root) / a, (-b + root) / a] {
                if !within(t) || found.iter().any(|(had, _)| *had == t) {
                    continue;
                }
                let at = add(p, mul(d, t));
                let on_sweep =
                    sweep >= TAU || (angle(sub(at, centre)) - start).rem_euclid(TAU) <= sweep;
                if on_sweep {
                    found.push((t, at));
                }
            }
            found
        }
    }
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Trim's step: a press notes the curve under it; a move follows the path, gathering in
/// Power trim each curve it crosses (the first crossing of each, the pressed curve's too);
/// the release decides (`released`). What it says, if it says anything.
fn trim_event(
    trim: &mut Trim,
    input: &Input<'_>,
    event: Event,
    requests: &mut Vec<Request>,
    preview: &mut Preview,
) -> Option<&'static str> {
    let tolerance = input.tolerance();
    let mut said = None;
    match event {
        Event::Press(at) => {
            let on = curve_at(input.read, at, tolerance);
            *trim = Trim::Pressed {
                along: on.is_some(),
                turned: 0.0,
                on,
                press: at,
                dragged: false,
                last: at,
                trail: vec![at],
                crossed: Vec::new(),
            };
        }
        Event::Move(at) | Event::Release(at) => {
            if let Trim::Pressed {
                on,
                press,
                dragged,
                along,
                turned,
                last,
                trail,
                crossed,
            } = trim
            {
                if length(sub(at, *press)) > tolerance {
                    *dragged = true;
                }
                *along &= on
                    .as_ref()
                    .is_some_and(|(_, _, curve)| off_carrier(*curve, at) <= tolerance);
                if *along && let Some((_, _, Curve::Round { centre, .. })) = on {
                    let step = angle(sub(at, *centre)) - angle(sub(*last, *centre));
                    *turned += (step + PI).rem_euclid(TAU) - PI;
                }
                if input.trim == TrimMode::Power {
                    cross_path(input.read, *last, at, crossed);
                }
                *last = at;
                trail.push(at);
                if trail.len() > TRAIL {
                    trail.drain(..trail.len() - TRAIL);
                }
            }
            if let Event::Release(at) = event {
                said = released(std::mem::take(trim), input, at, requests);
            }
        }
        Event::Escape | Event::Cancel => *trim = Trim::Idle,
        Event::DoubleClick(_) | Event::EndChain | Event::Enter => {}
    }
    *preview = trim_preview(trim, event);
    said
}

/// The curves the path's step from `from` to `to` crosses, in the order it crosses them,
/// each added to `crossed` with the place, if the path hasn't crossed it before.
fn cross_path(
    read: &SketchRead,
    from: [f64; 2],
    to: [f64; 2],
    crossed: &mut Vec<(ElementId, [f64; 2])>,
) {
    let had: BTreeSet<ElementId> = crossed.iter().map(|(id, _)| id.clone()).collect();
    let mut found: Vec<(f64, &ElementId, [f64; 2])> = Vec::new();
    let geometry = Geometry::of(read);
    for (id, curve, _) in geometry.curves() {
        if had.contains(id) {
            continue;
        }
        if let Some((t, at)) = crossings(from, to, curve).into_iter().next() {
            found.push((t, id, at));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    crossed.extend(found.into_iter().map(|(_, id, at)| (id.clone(), at)));
}

/// What trim's release at `at` asks, the press `trim` held. A click trims the curve it's
/// on there. In Power trim, a drag from a line or an arc that kept to its line or round
/// and ends past one of its ends extends that end to the pointer, put on it; any other
/// drag trims each curve it crossed, where it crossed it. In Trim to closest, a drag from a
/// curve extends the end it heads toward to the next curve.
fn released(
    trim: Trim,
    input: &Input<'_>,
    at: [f64; 2],
    requests: &mut Vec<Request>,
) -> Option<&'static str> {
    let Trim::Pressed {
        on,
        press,
        dragged,
        along,
        turned,
        crossed,
        ..
    } = trim
    else {
        return None;
    };
    if !dragged {
        let Some((curve, _, _)) = on else {
            return Some(NOTHING_TO_TRIM);
        };
        requests.push(Request::Trim { curve, pick: press });
        return None;
    }
    match (input.trim, on) {
        (TrimMode::Power, on) => {
            let past = on.filter(|_| along).and_then(|(curve, _, geometry)| {
                let end = past_end(geometry, press, turned, at)?;
                let to = onto(geometry, at)?;
                let by = [Coordinate::new(to[0])?, Coordinate::new(to[1])?];
                Some((curve, end, by))
            });
            if let Some((curve, end, by)) = past {
                requests.push(Request::Extend {
                    curve,
                    end,
                    by: ExtendBy::Point(by),
                    from: ExtendFrom::Trim,
                });
                return None;
            }
            if crossed.is_empty() {
                return Some(NOTHING_TO_TRIM);
            }
            requests.extend(
                crossed
                    .into_iter()
                    .map(|(curve, pick)| Request::Trim { curve, pick }),
            );
            None
        }
        (TrimMode::Closest, None) => Some(NOTHING_TO_TRIM),
        (TrimMode::Closest, Some((_, ElementKind::Circle, _))) => Some(NO_END_TO_EXTEND),
        (TrimMode::Closest, Some((curve, _, geometry))) => {
            requests.push(Request::Extend {
                curve,
                end: headed(geometry, press, at),
                by: ExtendBy::NextCurve,
                from: ExtendFrom::Trim,
            });
            None
        }
    }
}

/// How far `at` is from `curve`'s carrier: a line's whole line, an arc's or a circle's
/// round.
fn off_carrier(curve: Curve, at: [f64; 2]) -> f64 {
    match curve {
        Curve::Line { from, to } => length(sub(at, foot(at, from, to))),
        Curve::Round { centre, radius, .. } => (length(sub(at, centre)) - radius).abs(),
    }
}

/// The end of a line or an arc a drag along it went past: a line's end if `at`, put on its
/// line, lies beyond `to`, its start if before `from`; an arc's end if the angle of `press`
/// within its sweep, with the angle the drag `turned` round it added, passes the sweep, its
/// start if it goes below none. None within it, and for a circle, which has no end.
fn past_end(curve: Curve, press: [f64; 2], turned: f64, at: [f64; 2]) -> Option<End> {
    match curve {
        Curve::Line { from, to } => {
            let d = sub(to, from);
            let dd = dot(d, d);
            if dd <= 0.0 || !dd.is_finite() {
                return None;
            }
            let t = dot(sub(at, from), d) / dd;
            if t > 1.0 {
                Some(End::End)
            } else if t < 0.0 {
                Some(End::Start)
            } else {
                None
            }
        }
        Curve::Round {
            centre,
            start,
            sweep,
            ..
        } => {
            if sweep >= TAU {
                return None;
            }
            let from = (angle(sub(press, centre)) - start).rem_euclid(TAU);
            // A press past the arc's end, within the pick of it, is at its end.
            let from = if from > sweep {
                if from - sweep < TAU - from {
                    sweep
                } else {
                    0.0
                }
            } else {
                from
            };
            let reached = from + turned;
            if reached > sweep {
                Some(End::End)
            } else if reached < 0.0 {
                Some(End::Start)
            } else {
                None
            }
        }
    }
}

/// The end of a line or an arc a drag from `from` to `to` heads toward: along a line's
/// direction its end, against it its start; round an arc's way (counter-clockwise) its end,
/// against it its start.
fn headed(curve: Curve, from: [f64; 2], to: [f64; 2]) -> End {
    let ahead = match curve {
        Curve::Line { from: a, to: b } => dot(sub(to, from), sub(b, a)) > 0.0,
        Curve::Round { centre, .. } => cross(sub(from, centre), sub(to, centre)) > 0.0,
    };
    if ahead { End::End } else { End::Start }
}

/// What trim draws: its path, dotted, and where it crosses each curve; the pointer.
fn trim_preview(trim: &Trim, event: Event) -> Preview {
    let mut preview = Preview::default();
    if let Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) = event
    {
        preview.pointer = Some(at);
    }
    if let Trim::Pressed { trail, crossed, .. } = trim {
        preview.guides = trail.windows(2).map(|pair| [pair[0], pair[1]]).collect();
        preview.points = crossed.iter().map(|(_, at)| *at).collect();
    }
    preview
}

/// Split's step: a click on a line or an arc splits it there, put on it; a click on a
/// circle is its first place, and a second on the same circle splits it at both (a click on
/// another curve begins again there). What it says, if it says anything.
fn split_event(
    split: &mut Split,
    input: &Input<'_>,
    event: Event,
    requests: &mut Vec<Request>,
    preview: &mut Preview,
) -> Option<&'static str> {
    let tolerance = input.tolerance();
    let mut said = None;
    match event {
        Event::Press(at) => split.pressed = Some(at),
        Event::Release(at) => {
            if let Some(press) = split.pressed.take()
                && length(sub(at, press)) <= tolerance
            {
                said = split_click(split, input, press, tolerance, requests);
            }
        }
        Event::Escape => *split = Split::default(),
        // The press dropped; a circle's first place, clicked before it, kept.
        Event::Cancel => split.pressed = None,
        Event::Move(_) | Event::DoubleClick(_) | Event::EndChain | Event::Enter => {}
    }
    let mut drawn = Preview::default();
    if let Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) = event
    {
        drawn.pointer = Some(at);
    }
    drawn.points = split.first.iter().map(|(_, at)| *at).collect();
    *preview = drawn;
    said
}

/// A split's click at `at`.
fn split_click(
    split: &mut Split,
    input: &Input<'_>,
    at: [f64; 2],
    tolerance: f64,
    requests: &mut Vec<Request>,
) -> Option<&'static str> {
    let Some((curve, kind, geometry)) = curve_at(input.read, at, tolerance) else {
        return Some(NOTHING_TO_SPLIT);
    };
    let Some(on) = onto(geometry, at) else {
        return Some(NOTHING_TO_SPLIT);
    };
    if kind != ElementKind::Circle {
        split.first = None;
        requests.push(Request::Split {
            curve,
            at: vec![on],
        });
        return None;
    }
    match split.first.take() {
        Some((first, first_at)) if first == curve => {
            requests.push(Request::Split {
                curve,
                at: vec![first_at, on],
            });
            None
        }
        _ => {
            split.first = Some((curve, on));
            Some(SECOND_ON_CIRCLE)
        }
    }
}

/// Extend's step (SolidWorks' Extend Entities): each place the pointer is given hovers
/// there ([`extend_hover`]); a press and its release within the pick of it is a click, which
/// asks the `sketch_extend` the hover at the press showed ([`extend_click`]), unless a
/// click's extend is still to be read. The tool stays on, as SolidWorks' does. What it says
/// now, if what it said changes: the hover's, then the click's.
fn extend_event(
    extend: &mut Extend,
    input: &Input<'_>,
    event: Event,
    requests: &mut Vec<Request>,
    preview: &mut Preview,
) -> Option<Option<String>> {
    let mut said = None;
    match event {
        Event::Press(at) | Event::Move(at) | Event::Release(at) => {
            let (hovered, hover_said) = extend_hover(input, at);
            *preview = hovered;
            said = Some(hover_said);
            if let Event::Press(at) = event {
                extend.pressed = Some(at);
            }
            if let Event::Release(at) = event
                && let Some(press) = extend.pressed.take()
                && length(sub(at, press)) <= input.tolerance()
                && extend.sent.is_none()
            {
                let asked = requests.len();
                said = Some(extend_click(input, press, requests));
                if requests.len() > asked {
                    extend.sent = Some(input.revision);
                }
            }
        }
        Event::Escape | Event::Cancel => {
            extend.pressed = None;
            *preview = Preview::default();
        }
        Event::DoubleClick(_) | Event::EndChain | Event::Enter => {}
    }
    said
}

/// Why Extend takes nothing at a place: nothing there it could take (no curve, or a pointer
/// on neither half), a circle, or the core's words for a curve it refuses (one of no length
/// or no sweep, or one that meets nothing past the end).
enum Unextended {
    Nothing,
    Circle,
    Refused(String),
}

/// What Extend takes with the pointer at `at`, for the hover and the click alike: the curve
/// trim and split would pick there, a line or an arc; the end of the half under the pointer
/// (the core's `extend_end`); and that end extended to the next curve, drawn (the core's
/// `extend_preview`). Or why it takes nothing.
fn extend_pick(input: &Input<'_>, at: [f64; 2]) -> Result<(ElementId, End, Preview), Unextended> {
    let refused = |error: silico_core::Error| Unextended::Refused(error.message().to_owned());
    let Some((curve, kind, _)) = curve_at(input.read, at, input.tolerance()) else {
        return Err(Unextended::Nothing);
    };
    if kind == ElementKind::Circle {
        return Err(Unextended::Circle);
    }
    let members = &input.read.members;
    let end = extend_end(members, &curve, at)
        .map_err(refused)?
        .ok_or(Unextended::Nothing)?;
    let preview = extend_preview(members, &curve, end).map_err(refused)?;
    Ok((curve, end, preview))
}

/// What Extend shows with the pointer at `at`: what it takes there, drawn, with the pointer;
/// where the core refuses it, the pointer alone and the core's words; over anything else (a
/// circle, a point, an axis, nothing), the pointer alone, and nothing said.
fn extend_hover(input: &Input<'_>, at: [f64; 2]) -> (Preview, Option<String>) {
    let (mut preview, said) = match extend_pick(input, at) {
        Ok((_, _, extended)) => (extended, None),
        Err(Unextended::Refused(words)) => (Preview::default(), Some(words)),
        Err(Unextended::Nothing | Unextended::Circle) => (Preview::default(), None),
    };
    preview.pointer = Some(at);
    (preview, said)
}

/// Extend's click at `at`: the `sketch_extend` of what it takes there to the next curve
/// (none said); else what's said: that there's nothing to extend there, that a circle has no
/// end, or the core's words.
fn extend_click(input: &Input<'_>, at: [f64; 2], requests: &mut Vec<Request>) -> Option<String> {
    match extend_pick(input, at) {
        Ok((curve, end, _)) => {
            requests.push(Request::Extend {
                curve,
                end,
                by: ExtendBy::NextCurve,
                from: ExtendFrom::Extend,
            });
            None
        }
        Err(Unextended::Nothing) => Some(NOTHING_TO_EXTEND.to_owned()),
        Err(Unextended::Circle) => Some(NO_END_TO_EXTEND.to_owned()),
        Err(Unextended::Refused(words)) => Some(words),
    }
}

#[cfg(test)]
mod tests;
