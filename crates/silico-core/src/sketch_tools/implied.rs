//! The relations a sketch already implies (docs/ports/sketch-tools.md, *Relations the sketch
//! already implies*; #278's G as amended, Fable, 2026-11-08): what a tool leaves out of the
//! relations it infers, before the pointer shows them, so what's shown is what's added.
//!
//! Two partitions over the sketch's points and the origin: **Y**, the points level with each
//! other, and **X**, the points plumb with each other, each a union-find keyed by [`Key`]. A
//! class is **fixed** when it holds the origin, or a point the read marks fully defined. They
//! are seeded from the sketch's constraints (the read's and the pending adds', as the view
//! merges them), each read for the equation it asserts:
//!
//! - Y: `horizontal(L)` joins L's ends; `horizontal_points(a, b)` joins a and b;
//!   `on(p, x_axis)` joins p and the origin; a driving `distance_y(a, b)` of exactly 0 joins
//!   a and b. X likewise, with `vertical`, `vertical_points`, `on(p, y_axis)`, and a driving
//!   `distance_x` of 0.
//! - Propagated to a fixpoint: `on(p, L)` or `midpoint(p, L)`, L a line whose ends share a
//!   class, puts p in that class.
//!
//! Nothing else is read: what the other forms imply is PlaneGCS's to judge, and the read
//! reports it. Then a relation a tool infers is judged, in the order the gesture makes them,
//! each kept merged before the next:
//!
//! - **Rule E.** One asserting two points level (`horizontal`, `horizontal_points`, an `on` of
//!   the x axis or of a level line) is left out when they're already in one Y class, or both
//!   their Y classes are fixed. X likewise.
//! - **Rule F.** An inferred `perpendicular`, `parallel`, or `tangent_at` is left out when
//!   every point it reads is fixed in both X and Y.
//!
//! A relation left out reads only values already fixed, or asserts an equality already held,
//! so the solver would have called it redundant or conflicting. The tool's own relations (a
//! rectangle's sides, a centre rectangle's `symmetric`, a tangent arc's `tangent_at`) are
//! never judged. Classes are compared by membership, and IDs are keys only: the result is a
//! function of the sketch, its read, and the gesture, never of the order IDs sort in.

use std::collections::{BTreeMap, BTreeSet};

use crate::document::{ElementId, Form, Length, Reference, Sketch};
use crate::sketch_states::{MemberState, SketchStates};

/// Whether a drawing tool would leave `form` out, inferred on `sketch`, whose read gives
/// `states`: the sketch already implies it, by Rule E or Rule F. What the tools judge each
/// relation they infer by; public so the rule can be checked against PlaneGCS (a relation
/// it leaves out, added, is redundant or conflicting there, or leaves the freedoms as they
/// were). A form the rules don't read (a dimension, `equal`, ...) is never implied.
pub fn implied_by(sketch: &Sketch, states: &SketchStates, form: &Form) -> bool {
    let implied = Implied::of(sketch, states);
    implied.implies(&implied.rel(form))
}

/// A point a relation reads: the origin, a point of the sketch, or one a gesture is placing
/// and hasn't named yet (its first, its second).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Key {
    Origin,
    Point(ElementId),
    Fresh(u8),
}

impl Key {
    /// The point `reference` names, if it names one: the origin, or an element (a point, if
    /// the sketch holds it as one; the forms read here name only points there).
    fn of(reference: &Reference) -> Option<Key> {
        match reference {
            Reference::Origin => Some(Key::Origin),
            Reference::Element(id) => Some(Key::Point(id.clone())),
            Reference::XAxis | Reference::YAxis => None,
        }
    }
}

/// Which partition: points level with each other (equal y), or plumb (equal x).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Level,
    Plumb,
}

/// One partition: each key's parent toward its class's root, and the roots of the classes
/// that are fixed. A key never seen is a class of its own, fixed if [`Classes::fixed_keys`]
/// says so.
#[derive(Debug, Clone, Default)]
struct Classes {
    parent: BTreeMap<Key, Key>,
    fixed: BTreeSet<Key>,
}

impl Classes {
    fn root(&self, key: &Key) -> Key {
        let mut at = key.clone();
        while let Some(up) = self.parent.get(&at) {
            if *up == at {
                break;
            }
            at = up.clone();
        }
        at
    }

    fn same(&self, a: &Key, b: &Key) -> bool {
        self.root(a) == self.root(b)
    }

    fn is_fixed(&self, key: &Key) -> bool {
        self.fixed.contains(&self.root(key))
    }

    /// `a`'s class and `b`'s made one, its root the lesser (so the classes are the same
    /// whatever order they're joined in), fixed if either was. Whether they were two.
    fn join(&mut self, a: &Key, b: &Key) -> bool {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra == rb {
            return false;
        }
        let (low, high) = if ra < rb { (ra, rb) } else { (rb, ra) };
        if self.fixed.remove(&high) {
            self.fixed.insert(low.clone());
        }
        self.parent.insert(high, low);
        true
    }
}

/// A relation, as the classes read it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Rel {
    /// Two points level (`axis` Level) or plumb.
    Same(Axis, Key, Key),
    /// A point on a line, or its middle: in the line's class while the line is level or plumb.
    OnLine(Key, ElementId),
    /// A relation that reads these points' values and asserts nothing the classes hold: a
    /// `perpendicular`, `parallel`, or `tangent_at` (Rule F).
    Reads(Vec<Key>),
    /// Anything else: kept, and nothing to the classes.
    Other,
}

/// The two partitions of a sketch, and what propagates through its lines.
#[derive(Debug, Clone, Default)]
pub(super) struct Implied {
    y: Classes,
    x: Classes,
    /// Each line's ends, by its ID.
    lines: BTreeMap<ElementId, (Key, Key)>,
    /// Each circle's and arc's centre, by its ID.
    centres: BTreeMap<ElementId, Key>,
    /// Each point on a line, or at its middle.
    on_lines: Vec<(Key, ElementId)>,
}

impl Implied {
    /// The classes of `sketch` (the view's: the read's members and the pending adds'), its
    /// points fixed as `states` marks them.
    pub(super) fn of(sketch: &Sketch, states: &SketchStates) -> Implied {
        let mut implied = Implied::default();
        implied.y.fixed.insert(Key::Origin);
        implied.x.fixed.insert(Key::Origin);
        for (id, state) in &states.points {
            if *state == MemberState::FullyDefined {
                implied.y.fixed.insert(Key::Point(id.clone()));
                implied.x.fixed.insert(Key::Point(id.clone()));
            }
        }
        for line in &sketch.lines {
            implied
                .lines
                .entry(line.id.clone())
                .or_insert((Key::Point(line.from.clone()), Key::Point(line.to.clone())));
        }
        for circle in &sketch.circles {
            implied
                .centres
                .entry(circle.id.clone())
                .or_insert(Key::Point(circle.center.clone()));
        }
        for arc in &sketch.arcs {
            implied
                .centres
                .entry(arc.id.clone())
                .or_insert(Key::Point(arc.center.clone()));
        }
        for constraint in &sketch.constraints {
            let rel = implied.rel(&constraint.form);
            implied.assert(&rel);
        }
        implied
    }

    /// What `form` asserts, as the classes read it, against the classes as they stand (an
    /// `on` of a level line is an equality only while the line is level).
    pub(super) fn rel(&self, form: &Form) -> Rel {
        let key = Key::of;
        let pair = |axis, a: &Reference, b: &Reference| match (key(a), key(b)) {
            (Some(a), Some(b)) => Rel::Same(axis, a, b),
            _ => Rel::Other,
        };
        match form {
            Form::Horizontal { line } => self.line_ends(Axis::Level, line),
            Form::Vertical { line } => self.line_ends(Axis::Plumb, line),
            Form::HorizontalPoints { a, b } => pair(Axis::Level, a, b),
            Form::VerticalPoints { a, b } => pair(Axis::Plumb, a, b),
            Form::DistanceY {
                a,
                b,
                value,
                driving: true,
            } if *value == Length::default() => pair(Axis::Level, a, b),
            Form::DistanceX {
                a,
                b,
                value,
                driving: true,
            } if *value == Length::default() => pair(Axis::Plumb, a, b),
            Form::On { point, curve } => match key(point) {
                Some(p) => self.on(p, curve),
                None => Rel::Other,
            },
            Form::Midpoint {
                point,
                line: Reference::Element(line),
            } => match key(point) {
                Some(p) if self.lines.contains_key(line) => Rel::OnLine(p, line.clone()),
                _ => Rel::Other,
            },
            Form::Perpendicular { a, b }
            | Form::Parallel { a, b }
            | Form::TangentAt { a, b, .. } => match (self.reads(a), self.reads(b)) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Rel::Reads(a)
                }
                _ => Rel::Other,
            },
            _ => Rel::Other,
        }
    }

    /// The point `p` on `curve`: level with the origin on the x axis, plumb with it on the y
    /// axis, on a line of the sketch; anything else (a circle, an arc) nothing to the
    /// classes.
    pub(super) fn on(&self, p: Key, curve: &Reference) -> Rel {
        match curve {
            Reference::XAxis => Rel::Same(Axis::Level, p, Key::Origin),
            Reference::YAxis => Rel::Same(Axis::Plumb, p, Key::Origin),
            Reference::Element(line) if self.lines.contains_key(line) => {
                Rel::OnLine(p, line.clone())
            }
            _ => Rel::Other,
        }
    }

    /// A line's ends made level or plumb, if it's a line of the sketch.
    fn line_ends(&self, axis: Axis, line: &Reference) -> Rel {
        match line {
            Reference::Element(id) => match self.lines.get(id) {
                Some((a, b)) => Rel::Same(axis, a.clone(), b.clone()),
                None => Rel::Other,
            },
            _ => Rel::Other,
        }
    }

    /// The points a relation of `curve`'s direction reads: a line's two ends, an arc's or a
    /// circle's centre; none for anything else (an axis, or a curve the sketch hasn't).
    pub(super) fn reads(&self, curve: &Reference) -> Option<Vec<Key>> {
        let Reference::Element(id) = curve else {
            return None;
        };
        if let Some((a, b)) = self.lines.get(id) {
            return Some(vec![a.clone(), b.clone()]);
        }
        self.centres.get(id).map(|c| vec![c.clone()])
    }

    /// Whether the classes already imply `rel` (Rule E), or it reads only values they hold
    /// fixed (Rule F): a relation the tool leaves out.
    pub(super) fn implies(&self, rel: &Rel) -> bool {
        match rel {
            Rel::Same(axis, a, b) => {
                let classes = self.classes(*axis);
                classes.same(a, b) || (classes.is_fixed(a) && classes.is_fixed(b))
            }
            Rel::OnLine(p, line) => self
                .level_or_plumb(line)
                .into_iter()
                .any(|(axis, end)| self.implies(&Rel::Same(axis, p.clone(), end))),
            Rel::Reads(keys) => {
                !keys.is_empty()
                    && keys
                        .iter()
                        .all(|k| self.y.is_fixed(k) && self.x.is_fixed(k))
            }
            Rel::Other => false,
        }
    }

    /// `rel` kept: merged into the classes, and what it lets through the lines propagated.
    pub(super) fn assert(&mut self, rel: &Rel) {
        match rel {
            Rel::Same(axis, a, b) => {
                if self.classes_mut(*axis).join(a, b) {
                    self.propagate();
                }
            }
            Rel::OnLine(p, line) => {
                self.on_lines.push((p.clone(), line.clone()));
                self.propagate();
            }
            Rel::Reads(_) | Rel::Other => {}
        }
    }

    /// The line `line`'s classes it's level or plumb in, each with one of its ends.
    fn level_or_plumb(&self, line: &ElementId) -> Vec<(Axis, Key)> {
        let Some((a, b)) = self.lines.get(line) else {
            return Vec::new();
        };
        [Axis::Level, Axis::Plumb]
            .into_iter()
            .filter(|axis| self.classes(*axis).same(a, b))
            .map(|axis| (axis, a.clone()))
            .collect()
    }

    /// Each point on a line put in each class the line is level or plumb in, until nothing
    /// more joins.
    fn propagate(&mut self) {
        loop {
            let mut joined = false;
            for (p, line) in self.on_lines.clone() {
                for (axis, end) in self.level_or_plumb(&line) {
                    joined |= self.classes_mut(axis).join(&p, &end);
                }
            }
            if !joined {
                return;
            }
        }
    }

    fn classes(&self, axis: Axis) -> &Classes {
        match axis {
            Axis::Level => &self.y,
            Axis::Plumb => &self.x,
        }
    }

    fn classes_mut(&mut self, axis: Axis) -> &mut Classes {
        match axis {
            Axis::Level => &mut self.y,
            Axis::Plumb => &mut self.x,
        }
    }
}
