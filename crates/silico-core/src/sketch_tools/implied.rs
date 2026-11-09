//! The relations a sketch already implies (docs/ports/sketch-tools.md, *Relations the sketch
//! already implies*; #278's G as amended, Fable, 2026-11-08): what a tool leaves out of the
//! relations it infers, before the pointer shows them, so what's shown is what's added.
//!
//! Two partitions over the sketch's points and the origin: **Y**, the points level with each
//! other, and **X**, the points plumb with each other, each a union-find keyed by [`Key`]. A
//! class is **fixed** when it holds the origin, or a point the read marks fully defined. They
//! are built from the sketch's constraints (the read's and the pending adds', as the view
//! merges them), each read for the equation it asserts:
//!
//! - Y: `horizontal(L)` joins L's ends; `horizontal_points(a, b)` joins a and b;
//!   `on(p, x_axis)` joins p and the origin; a driving `distance_y(a, b)` of exactly 0 joins
//!   a and b. X likewise, with `vertical`, `vertical_points`, `on(p, y_axis)`, and a driving
//!   `distance_x` of 0.
//! - Through the lines, to a fixpoint: `on(p, L)` or `midpoint(p, L)`, L a line whose ends
//!   share a class, puts p in that class. Each line is taken up once in each partition, when
//!   a join first puts its ends in one class, and each point on it then joined to it, so the
//!   work is about linear in the constraints, never a pass over every point on a line for
//!   each join.
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

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::document::sketch_edit::Members;
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

/// The classes of a tool's newest view, built once for it, the first time a relation is
/// judged on it, and kept while what they're built from stays the same: the read's sketch,
/// the adds pending on it, and the read's states. Each judgement copies them, to merge a
/// gesture's own relations into.
#[derive(Debug, Default)]
pub(super) struct ViewClasses {
    of: Option<(Sketch, Members, SketchStates)>,
    classes: OnceCell<Implied>,
}

impl ViewClasses {
    /// Kept for a view of `sketch` with `pending` on it, whose read gives `states`: let go,
    /// to be built again, if they were another view's.
    pub(super) fn keep_for(&mut self, sketch: &Sketch, pending: &Members, states: &SketchStates) {
        let same = self
            .of
            .as_ref()
            .is_some_and(|(s, p, st)| s == sketch && p == pending && st == states);
        if !same {
            *self = ViewClasses {
                of: Some((sketch.clone(), pending.clone(), states.clone())),
                classes: OnceCell::new(),
            };
        }
    }

    /// The classes of `view` (the sketch [`keep_for`](Self::keep_for) was given, with its
    /// pending adds), its points fixed as `states` marks them: built the first time asked.
    pub(super) fn of(&self, view: &Sketch, states: &SketchStates) -> &Implied {
        self.classes.get_or_init(|| Implied::of(view, states))
    }
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

impl Axis {
    const BOTH: [Axis; 2] = [Axis::Level, Axis::Plumb];
}

/// One partition, a union-find by size: each key's slot, each slot's parent toward its
/// class's root, and, for each root, its class's members and whether it's fixed. A key never
/// seen is a class of its own, not fixed (the origin, and each point the read marks fully
/// defined, are seen as the classes are made: [`Classes::fix`]).
#[derive(Debug, Clone)]
struct Classes {
    slots: BTreeMap<Key, usize>,
    keys: Vec<Key>,
    parent: Vec<usize>,
    members: Vec<Vec<usize>>,
    fixed: Vec<bool>,
}

impl Classes {
    /// The origin's class, fixed, and no other.
    fn new() -> Classes {
        let mut classes = Classes {
            slots: BTreeMap::new(),
            keys: Vec::new(),
            parent: Vec::new(),
            members: Vec::new(),
            fixed: Vec::new(),
        };
        classes.fix(&Key::Origin);
        classes
    }

    /// `key`'s slot, a class of its own if it had none.
    fn slot(&mut self, key: &Key) -> usize {
        if let Some(&slot) = self.slots.get(key) {
            return slot;
        }
        let slot = self.keys.len();
        self.slots.insert(key.clone(), slot);
        self.keys.push(key.clone());
        self.parent.push(slot);
        self.members.push(vec![slot]);
        self.fixed.push(false);
        slot
    }

    /// The root of `slot`'s class: at most a logarithm of the keys away, as each join hangs
    /// the smaller class under the larger.
    fn find(&self, mut slot: usize) -> usize {
        while let Some(&up) = self.parent.get(slot)
            && up != slot
        {
            slot = up;
        }
        slot
    }

    fn root(&self, key: &Key) -> Option<usize> {
        self.slots.get(key).map(|&slot| self.find(slot))
    }

    fn same(&self, a: &Key, b: &Key) -> bool {
        a == b || matches!((self.root(a), self.root(b)), (Some(x), Some(y)) if x == y)
    }

    fn is_fixed(&self, key: &Key) -> bool {
        self.root(key)
            .is_some_and(|root| self.fixed.get(root).copied().unwrap_or(false))
    }

    /// `key`'s class fixed, at its root.
    fn fix(&mut self, key: &Key) {
        let slot = self.slot(key);
        let root = self.find(slot);
        if let Some(fixed) = self.fixed.get_mut(root) {
            *fixed = true;
        }
    }

    /// `a`'s class and `b`'s made one, the smaller hung under the larger, fixed if either
    /// was: the keys that were the smaller's, if they were two (which root a class has
    /// depends on the order of its joins, never who's in it).
    fn join(&mut self, a: &Key, b: &Key) -> Option<Vec<Key>> {
        let (sa, sb) = (self.slot(a), self.slot(b));
        let (ra, rb) = (self.find(sa), self.find(sb));
        if ra == rb {
            return None;
        }
        let size = |root: usize| self.members.get(root).map_or(0, Vec::len);
        let (big, small) = if size(ra) >= size(rb) {
            (ra, rb)
        } else {
            (rb, ra)
        };
        let moved = self.members.get_mut(small).map(std::mem::take)?;
        let fixed = self.fixed.get(small).copied().unwrap_or(false);
        if let Some(parent) = self.parent.get_mut(small) {
            *parent = big;
        }
        if let Some(was) = self.fixed.get_mut(big) {
            *was |= fixed;
        }
        let keys = moved
            .iter()
            .filter_map(|&slot| self.keys.get(slot).cloned())
            .collect();
        if let Some(members) = self.members.get_mut(big) {
            members.extend(moved);
        }
        Some(keys)
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

/// The two partitions of a sketch, and what joins through its lines.
#[derive(Debug, Clone)]
pub(super) struct Implied {
    y: Classes,
    x: Classes,
    /// Each line's ends, by its ID.
    lines: BTreeMap<ElementId, (Key, Key)>,
    /// Each circle's and arc's centre, by its ID.
    centres: BTreeMap<ElementId, Key>,
    /// The lines each point ends.
    ends: BTreeMap<Key, Vec<ElementId>>,
    /// The points on each line, or at its middle.
    on: BTreeMap<ElementId, Vec<Key>>,
    /// The lines each partition holds level (Y) or plumb (X): their ends in one class, and
    /// each point on them joined to it.
    flat_y: BTreeSet<ElementId>,
    flat_x: BTreeSet<ElementId>,
}

impl Implied {
    /// The classes of `sketch` (the view's: the read's members and the pending adds'), its
    /// points fixed as `states` marks them.
    pub(super) fn of(sketch: &Sketch, states: &SketchStates) -> Implied {
        let mut implied = Implied {
            y: Classes::new(),
            x: Classes::new(),
            lines: BTreeMap::new(),
            centres: BTreeMap::new(),
            ends: BTreeMap::new(),
            on: BTreeMap::new(),
            flat_y: BTreeSet::new(),
            flat_x: BTreeSet::new(),
        };
        for (id, state) in &states.points {
            if *state == MemberState::FullyDefined {
                let key = Key::Point(id.clone());
                implied.y.fix(&key);
                implied.x.fix(&key);
            }
        }
        for line in &sketch.lines {
            implied.line(
                &line.id,
                Key::Point(line.from.clone()),
                Key::Point(line.to.clone()),
            );
        }
        for (id, center) in sketch
            .circles
            .iter()
            .map(|c| (&c.id, &c.center))
            .chain(sketch.arcs.iter().map(|a| (&a.id, &a.center)))
        {
            implied
                .centres
                .entry(id.clone())
                .or_insert(Key::Point(center.clone()));
        }
        for constraint in &sketch.constraints {
            let rel = implied.rel(&constraint.form);
            implied.assert(&rel);
        }
        implied
    }

    /// The line `id` from `a` to `b` (a gesture's own, not yet named, as well as the
    /// sketch's), if there's none of that ID yet; level or plumb at once if its ends already
    /// share a class.
    pub(super) fn line(&mut self, id: &ElementId, a: Key, b: Key) {
        if self.lines.contains_key(id) {
            return;
        }
        for end in [&a, &b] {
            self.ends.entry(end.clone()).or_default().push(id.clone());
        }
        self.lines.insert(id.clone(), (a.clone(), b.clone()));
        for axis in Axis::BOTH {
            if self.classes(axis).same(&a, &b) {
                for (p, end) in self.flatten(axis, id) {
                    self.join(axis, p, end);
                }
            }
        }
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
            Rel::OnLine(p, line) => {
                let Some((end, other)) = self.lines.get(line) else {
                    return false;
                };
                Axis::BOTH.into_iter().any(|axis| {
                    self.classes(axis).same(end, other)
                        && self.implies(&Rel::Same(axis, p.clone(), end.clone()))
                })
            }
            Rel::Reads(keys) => {
                !keys.is_empty()
                    && keys
                        .iter()
                        .all(|k| self.y.is_fixed(k) && self.x.is_fixed(k))
            }
            Rel::Other => false,
        }
    }

    /// `rel` held: merged into the classes, and what it lets through the lines joined.
    pub(super) fn assert(&mut self, rel: &Rel) {
        match rel {
            Rel::Same(axis, a, b) => self.join(*axis, a.clone(), b.clone()),
            Rel::OnLine(p, line) => {
                let Some((end, _)) = self.lines.get(line).cloned() else {
                    return;
                };
                #[cfg(test)]
                super::infer::work::visit();
                self.on.entry(line.clone()).or_default().push(p.clone());
                for axis in Axis::BOTH {
                    if self.flat(axis).contains(line) {
                        self.join(axis, p.clone(), end.clone());
                    }
                }
            }
            Rel::Reads(_) | Rel::Other => {}
        }
    }

    /// `a`'s and `b`'s classes in `axis` made one, and each join that lets through the
    /// lines: a line whose ends that puts in one class is taken up (each line once), and each
    /// point on it joined to it, and so on, until nothing more joins. Only the lines ending at
    /// a point of the smaller class are looked at, so each point's lines are looked at as
    /// often as its class at least doubles.
    fn join(&mut self, axis: Axis, a: Key, b: Key) {
        let mut work = vec![(a, b)];
        while let Some((a, b)) = work.pop() {
            let Some(moved) = self.classes_mut(axis).join(&a, &b) else {
                continue;
            };
            let mut taken = Vec::new();
            for key in &moved {
                for line in self.ends.get(key).into_iter().flatten() {
                    #[cfg(test)]
                    super::infer::work::visit();
                    if let Some((e, f)) = self.lines.get(line)
                        && !self.flat(axis).contains(line)
                        && self.classes(axis).same(e, f)
                    {
                        taken.push(line.clone());
                    }
                }
            }
            for line in taken {
                work.extend(self.flatten(axis, &line));
            }
        }
    }

    /// The line `line` held level (Y) or plumb (X) in `axis`, if it wasn't already: the
    /// joins each point on it then needs, to its first end.
    fn flatten(&mut self, axis: Axis, line: &ElementId) -> Vec<(Key, Key)> {
        if !self.flat_mut(axis).insert(line.clone()) {
            return Vec::new();
        }
        let Some((end, _)) = self.lines.get(line) else {
            return Vec::new();
        };
        self.on
            .get(line)
            .into_iter()
            .flatten()
            .map(|p| {
                #[cfg(test)]
                super::infer::work::visit();
                (p.clone(), end.clone())
            })
            .collect()
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

    fn flat(&self, axis: Axis) -> &BTreeSet<ElementId> {
        match axis {
            Axis::Level => &self.flat_y,
            Axis::Plumb => &self.flat_x,
        }
    }

    fn flat_mut(&mut self, axis: Axis) -> &mut BTreeSet<ElementId> {
        match axis {
            Axis::Level => &mut self.flat_y,
            Axis::Plumb => &mut self.flat_x,
        }
    }
}
