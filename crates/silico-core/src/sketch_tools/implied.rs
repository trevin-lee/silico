//! The relations a sketch already implies (docs/ports/sketch-tools.md, *Relations the sketch
//! already implies*; #278's G as amended, Fable, 2026-11-08): what a tool leaves out of the
//! relations it infers, before the pointer shows them, so what's shown is what's added.
//!
//! Two partitions over the sketch's points and the origin: **Y**, the points sharing a y
//! (level with each other), and **X**, the points sharing an x (plumb), each a union-find
//! keyed by [`Key`]. A class is **fixed** when it holds the origin, or a point the read marks
//! fully defined. They're built from the sketch's constraints (the read's and the pending
//! adds', as the view merges them), each read for the equation it asserts:
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
//! reports it. Then a relation a tool infers is judged ([`Implied::admit`]), in the order the
//! gesture makes them, each kept merged before the next:
//!
//! - **Rule E.** One asserting two points level (`horizontal`, `horizontal_points`, an `on` of
//!   the x axis or of a level line) is left out when they're already in one Y class, or both
//!   their Y classes are fixed. X likewise. A `midpoint` asserts both its point's coordinates,
//!   which no class holds whole: it's only partly implied, and never left out.
//! - **Rule F.** An inferred `perpendicular`, `parallel`, or `tangent_at` is left out when
//!   every point it reads is fixed in both X and Y (a line's two ends, an arc's or a circle's
//!   centre, and a `tangent_at`'s point).
//!
//! A relation left out reads only values already fixed, or asserts an equality already held,
//! so the solver would have called it redundant or conflicting. The tool's own relations (a
//! rectangle's sides, a centre rectangle's `symmetric`, a tangent arc's `tangent_at`) are
//! never judged. Classes are compared by membership, and IDs are keys only: the result is a
//! function of the sketch, its read, and the gesture, never of the order IDs sort in or
//! classes join in.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::document::{ElementId, Form, Length, Reference, Sketch};
use crate::sketch_states::{MemberState, SketchStates};

/// Whether a drawing tool would leave `form` out, inferred on `sketch`, whose read gives
/// `states`: the sketch already implies it, by Rule E or Rule F. What the tools judge each
/// relation they infer by; public so the rule can be checked against PlaneGCS (a relation
/// it leaves out, added, is redundant or conflicting there, or leaves the freedoms as they
/// were). A form the rules don't read (a dimension, `equal`, a `midpoint`, ...) is never
/// implied.
pub fn implied_by(sketch: &Sketch, states: &SketchStates, form: &Form) -> bool {
    let implied = Implied::of(sketch, states);
    implied.implies(&implied.rel(form))
}

/// The classes of a tool's newest view, built once for it, the first time a relation is
/// judged on it, from what [`keep_for`](Self::keep_for) keeps: the view (the read's sketch
/// with the adds pending on it merged) and the read's states; kept while those stay the
/// same. Each judgement copies them, to merge a gesture's own relations into.
#[derive(Debug, Default)]
pub(super) struct ViewClasses {
    /// The view and its read's states: none before the first.
    of: Option<(Sketch, SketchStates)>,
    classes: OnceCell<Implied>,
}

impl ViewClasses {
    /// Kept for `view`, whose read gives `states`: let go, to be built again, if they were
    /// another view's.
    pub(super) fn keep_for(&mut self, view: &Sketch, states: &SketchStates) {
        let same = self
            .of
            .as_ref()
            .is_some_and(|(v, st)| v == view && st == states);
        if !same {
            *self = ViewClasses {
                of: Some((view.clone(), states.clone())),
                classes: OnceCell::new(),
            };
        }
    }

    /// The classes of the view [`keep_for`](Self::keep_for) was last given, its points fixed
    /// as its states mark them: built the first time asked. Before any, the origin's alone.
    pub(super) fn of(&self) -> &Implied {
        self.classes.get_or_init(|| match &self.of {
            Some((view, states)) => Implied::of(view, states),
            None => Implied::new(),
        })
    }
}

/// A point a relation reads: the origin, or a point, of the sketch or one a gesture is
/// placing (named by [`Unnamed`](super::infer::Unnamed) till it has an ID, none the view
/// holds, so a class of its own until a relation joins it).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Key {
    Origin,
    Point(ElementId),
}

impl Key {
    /// The point `reference` names, if it names one: the origin, or an element (a point, if
    /// the sketch holds it as one; the forms read here name only points there).
    pub(super) fn of(reference: &Reference) -> Option<Key> {
        match reference {
            Reference::Origin => Some(Key::Origin),
            Reference::Element(id) => Some(Key::Point(id.clone())),
            Reference::XAxis | Reference::YAxis => None,
        }
    }
}

/// Which partition: the points sharing a y (level with each other), or sharing an x
/// (plumb).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Partition {
    Y,
    X,
}

impl Partition {
    const BOTH: [Partition; 2] = [Partition::Y, Partition::X];
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
pub(super) enum ClassRel {
    /// Two points sharing a y (`Partition::Y`) or an x.
    Same(Partition, Key, Key),
    /// A point on a line: in the line's class while the line is level or plumb. The ID may
    /// name no line the classes hold (a circle, an arc, or a line not taken up): every reader
    /// takes that as `Other`.
    OnLine(Key, ElementId),
    /// A point at a line's middle: in the line's class as one on it is, and only partly
    /// implied (its place along the line no class holds), so never left out. The ID may name
    /// no line the classes hold, read as `Other`, as `OnLine`'s.
    Midpoint(Key, ElementId),
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
    /// The classes of no sketch: the origin's alone, fixed in each partition.
    fn new() -> Implied {
        Implied {
            y: Classes::new(),
            x: Classes::new(),
            lines: BTreeMap::new(),
            centres: BTreeMap::new(),
            ends: BTreeMap::new(),
            on: BTreeMap::new(),
            flat_y: BTreeSet::new(),
            flat_x: BTreeSet::new(),
        }
    }

    /// The classes of `sketch` (the view's: the read's members and the pending adds'), its
    /// points fixed as `states` marks them.
    pub(super) fn of(sketch: &Sketch, states: &SketchStates) -> Implied {
        let mut implied = Implied::new();
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
            implied.merge(&rel);
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
        for partition in Partition::BOTH {
            if self.classes(partition).same(&a, &b) {
                for (p, end) in self.flatten(partition, id) {
                    self.join(partition, p, end);
                }
            }
        }
    }

    /// What `form` asserts, as the classes read it, each point it names known by its
    /// [`Key`]: it reads no classes, only which lines and curves the sketch has.
    pub(super) fn rel(&self, form: &Form) -> ClassRel {
        let key = Key::of;
        let pair = |partition, a: &Reference, b: &Reference| match (key(a), key(b)) {
            (Some(a), Some(b)) => ClassRel::Same(partition, a, b),
            _ => ClassRel::Other,
        };
        match form {
            Form::Horizontal { line } => self.line_ends(Partition::Y, line),
            Form::Vertical { line } => self.line_ends(Partition::X, line),
            Form::HorizontalPoints { a, b } => pair(Partition::Y, a, b),
            Form::VerticalPoints { a, b } => pair(Partition::X, a, b),
            Form::DistanceY {
                a,
                b,
                value,
                driving: true,
            } if *value == Length::default() => pair(Partition::Y, a, b),
            Form::DistanceX {
                a,
                b,
                value,
                driving: true,
            } if *value == Length::default() => pair(Partition::X, a, b),
            Form::On { point, curve } => match (key(point), curve) {
                (Some(p), Reference::XAxis) => ClassRel::Same(Partition::Y, p, Key::Origin),
                (Some(p), Reference::YAxis) => ClassRel::Same(Partition::X, p, Key::Origin),
                // On a line the classes don't hold (a circle, an arc, or a line not taken
                // up): `implies` and `merge` take nothing from it, as from `Other`.
                (Some(p), Reference::Element(line)) => ClassRel::OnLine(p, line.clone()),
                _ => ClassRel::Other,
            },
            Form::Midpoint {
                point,
                line: Reference::Element(line),
            } => match key(point) {
                // Of a line the classes don't hold: taken as `Other`, as `OnLine`'s.
                Some(p) => ClassRel::Midpoint(p, line.clone()),
                None => ClassRel::Other,
            },
            Form::Perpendicular { a, b } | Form::Parallel { a, b } => {
                match (self.reads(a), self.reads(b)) {
                    (Some(mut a), Some(b)) => {
                        a.extend(b);
                        ClassRel::Reads(a)
                    }
                    _ => ClassRel::Other,
                }
            }
            Form::TangentAt { a, b, at, .. } => match (self.reads(a), self.reads(b), key(at)) {
                (Some(mut a), Some(b), Some(at)) => {
                    a.extend(b);
                    a.push(at);
                    ClassRel::Reads(a)
                }
                _ => ClassRel::Other,
            },
            _ => ClassRel::Other,
        }
    }

    /// A line's ends made level or plumb, if it's a line the classes know.
    fn line_ends(&self, partition: Partition, line: &Reference) -> ClassRel {
        match line {
            Reference::Element(id) => match self.lines.get(id) {
                Some((a, b)) => ClassRel::Same(partition, a.clone(), b.clone()),
                None => ClassRel::Other,
            },
            _ => ClassRel::Other,
        }
    }

    /// The points a relation of `curve`'s direction reads: a line's two ends, an arc's or a
    /// circle's centre; none for anything else (an axis, or a curve the sketch hasn't).
    fn reads(&self, curve: &Reference) -> Option<Vec<Key>> {
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
    pub(super) fn implies(&self, rel: &ClassRel) -> bool {
        match rel {
            ClassRel::Same(partition, a, b) => {
                let classes = self.classes(*partition);
                classes.same(a, b) || (classes.is_fixed(a) && classes.is_fixed(b))
            }
            ClassRel::OnLine(p, line) => {
                let Some((end, other)) = self.lines.get(line) else {
                    return false;
                };
                Partition::BOTH.into_iter().any(|partition| {
                    self.classes(partition).same(end, other)
                        && self.implies(&ClassRel::Same(partition, p.clone(), end.clone()))
                })
            }
            ClassRel::Reads(keys) => {
                !keys.is_empty()
                    && keys
                        .iter()
                        .all(|k| self.y.is_fixed(k) && self.x.is_fixed(k))
            }
            ClassRel::Midpoint(..) | ClassRel::Other => false,
        }
    }

    /// `rel`, a relation a tool infers, judged: whether it's kept (the classes don't already
    /// imply it), merged into them if it is, so the gesture's next relation is judged with
    /// it held.
    pub(super) fn admit(&mut self, rel: &ClassRel) -> bool {
        if self.implies(rel) {
            return false;
        }
        self.merge(rel);
        true
    }

    /// `rel` held: merged into the classes, and what it lets through the lines joined.
    pub(super) fn merge(&mut self, rel: &ClassRel) {
        match rel {
            ClassRel::Same(partition, a, b) => self.join(*partition, a.clone(), b.clone()),
            ClassRel::OnLine(p, line) | ClassRel::Midpoint(p, line) => {
                let Some((end, _)) = self.lines.get(line).cloned() else {
                    return;
                };
                #[cfg(test)]
                super::infer::work::visit();
                self.on.entry(line.clone()).or_default().push(p.clone());
                for partition in Partition::BOTH {
                    if self.flat(partition).contains(line) {
                        self.join(partition, p.clone(), end.clone());
                    }
                }
            }
            ClassRel::Reads(_) | ClassRel::Other => {}
        }
    }

    /// `a`'s and `b`'s classes in `partition` made one, and each join that lets through the
    /// lines: a line whose ends that puts in one class is taken up (each line once), and each
    /// point on it joined to it, and so on, until nothing more joins. Only the lines ending at
    /// a point of the smaller class are looked at, so each point's lines are looked at as
    /// often as its class at least doubles.
    fn join(&mut self, partition: Partition, a: Key, b: Key) {
        let mut work = vec![(a, b)];
        while let Some((a, b)) = work.pop() {
            let Some(moved) = self.classes_mut(partition).join(&a, &b) else {
                continue;
            };
            let mut taken = Vec::new();
            for key in &moved {
                for line in self.ends.get(key).into_iter().flatten() {
                    #[cfg(test)]
                    super::infer::work::visit();
                    if let Some((e, f)) = self.lines.get(line)
                        && !self.flat(partition).contains(line)
                        && self.classes(partition).same(e, f)
                    {
                        taken.push(line.clone());
                    }
                }
            }
            for line in taken {
                work.extend(self.flatten(partition, &line));
            }
        }
    }

    /// The line `line` held level (Y) or plumb (X) in `partition`, if it wasn't already: the
    /// joins each point on it then needs, to its first end.
    fn flatten(&mut self, partition: Partition, line: &ElementId) -> Vec<(Key, Key)> {
        if !self.flat_mut(partition).insert(line.clone()) {
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

    fn classes(&self, partition: Partition) -> &Classes {
        match partition {
            Partition::Y => &self.y,
            Partition::X => &self.x,
        }
    }

    fn classes_mut(&mut self, partition: Partition) -> &mut Classes {
        match partition {
            Partition::Y => &mut self.y,
            Partition::X => &mut self.x,
        }
    }

    fn flat(&self, partition: Partition) -> &BTreeSet<ElementId> {
        match partition {
            Partition::Y => &self.flat_y,
            Partition::X => &self.flat_x,
        }
    }

    fn flat_mut(&mut self, partition: Partition) -> &mut BTreeSet<ElementId> {
        match partition {
            Partition::Y => &mut self.flat_y,
            Partition::X => &mut self.flat_x,
        }
    }
}
