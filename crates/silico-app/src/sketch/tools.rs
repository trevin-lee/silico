//! Sketch mode's tools (M5.11b; docs/design/m5.0-app.md, section 9, *Sketching*): the
//! drawing tools, each the core's [`SketchTool`] (#335) driven through its API alone, with
//! no rule of its own copied here; and trim and split, the core's `sketch_trim`,
//! `sketch_split`, and `sketch_extend` asked from the pointer's picks. What a tool asks of
//! the core comes out as [`Request`]s, which the tab sends through its queue, never waited
//! on; the core's answer to an add is given back by its token ([`Tools::added`],
//! [`Tools::refused`]), in whatever order it comes.
//!
//! What's here of trim and split is only where the pointer is: which curve it's on (within
//! the pick tolerance, as sketch mode's selection hits one), where a drag's path crosses a
//! curve, and, for a split, the click put on its curve (the core refuses a position farther
//! than 1e-7 mm from it, decision 4). Where to cut, what's removed, and what's kept are the
//! core's.

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use silico_core::command::sketch::SketchRead;
use silico_core::command::{ExtendBy, Revision};
use silico_core::document::sketch_edit::Members;
use silico_core::document::{Coordinate, ElementId, ElementKind, FeatureId};
use silico_core::extend::End;
use silico_core::sketch_tools::{
    AddToken, Context, Event, Modifiers, NewIds, Preview, SketchTool, SystemIds, ToolKind,
};

use super::draw::{Curve, Geometry, add, angle, dot, foot, length, mul, sub};
use super::hit::distance;

/// The tool chosen: a drawing tool of the core's, trim, or split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Draw(ToolKind),
    Trim,
    Split,
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

/// What a tool asks of the core, in the order it asks: a drawing tool's `sketch_add`, with
/// its token, and trim's, split's, and extend's edits, each of a curve of the newest read.
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
    },
}

/// What a tool works against for one event: the newest read, of the sketch `sketch`, of the
/// model at `revision`; the pick tolerance there, in millimetres; and the palette's
/// settings it takes.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub read: &'a SketchRead,
    pub sketch: &'a FeatureId,
    pub revision: Revision,
    pub tolerance: f64,
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
    Trim(Trim),
    Split(Split),
}

/// Trim, between events: a press under way, with what it was on (a curve, its kind, and its
/// geometry then), where it was, whether it has left the pick tolerance of there, whether
/// every place since has stayed within it of that curve's line or round, the path's last
/// place and its trail (the last [`TRAIL`] places, for drawing), and the curves the path
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

impl Tools {
    /// The tool chosen, if one is.
    pub fn choice(&self) -> Option<Choice> {
        self.active.as_ref().map(|active| match active {
            Active::Draw(tool) => Choice::Draw(tool.kind()),
            Active::Trim(_) => Choice::Trim,
            Active::Split(_) => Choice::Split,
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
            Choice::Trim => Active::Trim(Trim::Idle),
            Choice::Split => Active::Split(Split::default()),
        });
        self.preview = Preview::default();
        self.said = None;
        self.last = None;
    }

    /// Whether a gesture is under way: a press held, a chain or an entity begun, or a
    /// circle's first split place clicked.
    pub fn busy(&self) -> bool {
        match &self.active {
            None => false,
            Some(Active::Draw(tool)) => tool.busy(),
            Some(Active::Trim(trim)) => matches!(trim, Trim::Pressed { .. }),
            Some(Active::Split(split)) => split.pressed.is_some() || split.first.is_some(),
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
        if let Some(Active::Draw(tool)) = &mut self.active {
            tool.added(token, revision);
        }
    }

    /// The core refused the add `token` names: the tool forgets it.
    pub fn refused(&mut self, token: AddToken) {
        if let Some(Active::Draw(tool)) = &mut self.active {
            tool.refused(token);
        }
    }

    /// Everything the tool added and no read has shown forgotten, with the gesture under
    /// way: for a sketch mode whose session went back to the file (a reload).
    pub fn forget_pending(&mut self) {
        if let Some(Active::Draw(tool)) = &mut self.active {
            tool.forget_pending();
        }
        if let Some(Active::Trim(trim)) = &mut self.active {
            *trim = Trim::Idle;
        }
        if let Some(Active::Split(split)) = &mut self.active {
            *split = Split::default();
        }
        self.preview = Preview::default();
    }

    /// Esc, with a tool on (section 9, *Line*: Esc ends a chain and leaves the tool): the
    /// gesture ended and the tool left. A drawing tool is told, with `input` if there's a
    /// read to give it. Whether there was a tool to leave.
    pub fn escape(&mut self, input: Option<&Input<'_>>) -> bool {
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
    /// hidden), or a release off the plane or with no read to give it: the gesture under
    /// way ended with nothing added, the tool kept, so no press outlives its release. The
    /// core's tools end one only by Esc, which is sent with `input`, its leaving the tool
    /// not taken; with no read to give it, the tool is started afresh
    /// ([`SketchTool::forget_pending`], which forgets its unanswered adds too: their points
    /// are found again in the next read).
    pub fn cancel(&mut self, input: Option<&Input<'_>>) {
        match (&mut self.active, input) {
            (Some(Active::Draw(tool)), Some(input)) => {
                tool.handle(
                    &input.context(),
                    self.ids.as_mut(),
                    Event::Escape,
                    Modifiers::default(),
                );
            }
            (Some(Active::Draw(tool)), None) => tool.forget_pending(),
            (Some(Active::Trim(trim)), _) => *trim = Trim::Idle,
            (Some(Active::Split(split)), Some(_)) => split.pressed = None,
            (Some(Active::Split(split)), None) => *split = Split::default(),
            (None, _) => {}
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
                if let Some(message) = &outcome.message {
                    self.said = Some(message.text());
                }
                if let (Some(mut members), Some(token)) = (outcome.add, outcome.token) {
                    if input.construction {
                        construction(&mut members);
                    }
                    self.said = None;
                    self.requests.push(Request::Add { token, members });
                }
                if outcome.left {
                    self.choose(None);
                }
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

/// `members`' curves drawn as construction (the palette's Construction, X): what the person
/// asked of the tool, which #335's tools have no setting for. Their points stay as made.
fn construction(members: &mut Members) {
    for line in &mut members.lines {
        line.construction = true;
    }
    for circle in &mut members.circles {
        circle.construction = true;
    }
    for arc in &mut members.arcs {
        arc.construction = true;
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
        Event::Escape => *trim = Trim::Idle,
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
                let end = past_end(geometry, at)?;
                let to = onto(geometry, at)?;
                let by = [Coordinate::new(to[0])?, Coordinate::new(to[1])?];
                Some((curve, end, by))
            });
            if let Some((curve, end, by)) = past {
                requests.push(Request::Extend {
                    curve,
                    end,
                    by: ExtendBy::Point(by),
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

/// The end of a line or an arc that `at`, put on its carrier, lies past: a line's end
/// beyond `to`, its start before `from`; an arc's, outside its sweep, the end nearer in
/// angle. None within it, and for a circle, which has no end.
fn past_end(curve: Curve, at: [f64; 2]) -> Option<End> {
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
            let turned = (angle(sub(at, centre)) - start).rem_euclid(TAU);
            if turned <= sweep {
                None
            } else if turned - sweep < TAU - turned {
                Some(End::End)
            } else {
                Some(End::Start)
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

#[cfg(test)]
mod tests;
