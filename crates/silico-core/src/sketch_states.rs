//! Each sketch member's definition state, as SolidWorks names them, from the solve's
//! diagnosis (docs/ports/sketcher.md, *Definition states (the sketch read, M5.10, open item
//! 32)*, as M5's open item 32 decided it, 2026-10-29): the sketch's `under_defined`,
//! `fully_defined`, `over_defined`, `no_solution`, or `invalid`; each point's and curve's
//! `under_defined`, `fully_defined`, or `over_defined`; each constraint's `satisfied`,
//! `conflicting`, `redundant`, `partially_redundant`, or `unsolved`; and the solve's problem,
//! the core's error, for a sketch whose solve was refused. The core decides them, so no
//! client maps unknowns to members.
//!
//! Every rule is decided here, each in a function of its own, so any one can change:
//! [`conflict_state`] and [`redundancy_state`] (the decision's 1 and 2), [`point_state`] and
//! [`curve_over_defined`] (3), [`constraint_state`] (A, `unsolved`), and [`EMPTY`] (B).
//! Which constraint of a dependent group is flagged, and as what, is PlaneGCS's choice as it
//! lists it (its popularity contest), never a rule here: the symmetric arc's case is the
//! solver's answer, reported as it gives it (the decision's 5).

use std::collections::{BTreeMap, BTreeSet};

use crate::document::{ElementId, Sketch, SketchConstraint};
use crate::sketch::{Diagnosis, SketchSolution};
use crate::{Error, ErrorCode};

/// A sketch's state: SolidWorks' five (docs/ux/solidworks.md, §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SketchState {
    /// Solved, with degrees of freedom left, and no constraint repeating others.
    UnderDefined,
    /// Solved, with none left and no constraint repeating others; or empty.
    FullyDefined,
    /// Constraints that conflict (refused), or that repeat others and agree (solved).
    OverDefined,
    /// No algorithm placed it, or its loads don't settle.
    NoSolution,
    /// Solved only to geometry that can't be built, or that breaks what its constraints
    /// mean.
    Invalid,
}

impl SketchState {
    /// The state's word: `under_defined`, `fully_defined`, `over_defined`, `no_solution`,
    /// or `invalid`.
    pub fn as_str(self) -> &'static str {
        match self {
            SketchState::UnderDefined => "under_defined",
            SketchState::FullyDefined => "fully_defined",
            SketchState::OverDefined => "over_defined",
            SketchState::NoSolution => "no_solution",
            SketchState::Invalid => "invalid",
        }
    }
}

/// A point's or a curve's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberState {
    /// Some value of it is free.
    UnderDefined,
    /// Every value of it is fixed.
    FullyDefined,
    /// A flagged constraint (in conflict, or repeating others) governs it.
    OverDefined,
}

impl MemberState {
    /// The state's word: `under_defined`, `fully_defined`, or `over_defined`.
    pub fn as_str(self) -> &'static str {
        match self {
            MemberState::UnderDefined => "under_defined",
            MemberState::FullyDefined => "fully_defined",
            MemberState::OverDefined => "over_defined",
        }
    }
}

/// A constraint's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConstraintState {
    /// On a solved sketch: independent of the others in the diagnosis, and met on the
    /// coordinates the solve gives (a reference dimension always, its value measured there).
    Satisfied,
    /// In conflict with others: PlaneGCS's, or decision 5's disagreeing redundancy.
    Conflicting,
    /// Every equation of it repeats others'.
    Redundant,
    /// Some equations of it repeat others'.
    PartiallyRedundant,
    /// On a sketch whose solve was refused, any constraint not flagged: there's no solution
    /// for it to be met on (SolidWorks' "Not Solved").
    Unsolved,
}

impl ConstraintState {
    /// The state's word: `satisfied`, `conflicting`, `redundant`, `partially_redundant`, or
    /// `unsolved`.
    pub fn as_str(self) -> &'static str {
        match self {
            ConstraintState::Satisfied => "satisfied",
            ConstraintState::Conflicting => "conflicting",
            ConstraintState::Redundant => "redundant",
            ConstraintState::PartiallyRedundant => "partially_redundant",
            ConstraintState::Unsolved => "unsolved",
        }
    }

    /// Whether the constraint is flagged: in conflict, or repeating others. A flagged
    /// constraint over-defines what it governs.
    pub fn flagged(self) -> bool {
        matches!(
            self,
            ConstraintState::Conflicting
                | ConstraintState::Redundant
                | ConstraintState::PartiallyRedundant
        )
    }
}

/// A sketch's states: its own, its degrees of freedom, each member's, by ID, and the solve's
/// problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SketchStates {
    pub state: SketchState,
    /// Degrees of freedom left, as PlaneGCS counts them: negative when a conflict
    /// over-determines the sketch. 0 for an empty sketch.
    pub dofs: i32,
    pub points: BTreeMap<ElementId, MemberState>,
    pub lines: BTreeMap<ElementId, MemberState>,
    pub circles: BTreeMap<ElementId, MemberState>,
    pub arcs: BTreeMap<ElementId, MemberState>,
    pub constraints: BTreeMap<ElementId, ConstraintState>,
    /// Why the solve was refused, as the core refused it (the error contract's object):
    /// present for every refused solve, `over_constrained` included; none when the sketch
    /// solved, or is empty.
    pub solve_problem: Option<Error>,
}

/// The decision's B: an empty sketch (no points, so no curves or constraints) is
/// `fully_defined`, with no degrees of freedom, as FreeCAD's "Empty sketch" is fully
/// constrained with DoF 0 (1.1.3). PlaneGCS's −1 for no unknowns is never read.
pub const EMPTY: (SketchState, i32) = (SketchState::FullyDefined, 0);

/// The states of `sketch`, from `diagnosis`, its solve's diagnosis
/// ([`Translation::diagnosis`](crate::sketch::Translation::diagnosis)), and `outcome`, the
/// read of the same answer: the one that stands (`Answer::outcome`). Points' and curves'
/// states come from the diagnosis whatever the outcome, so a refused sketch's members (given
/// as stored) have them too; the sketch's state, and whether a constraint not flagged is
/// `satisfied` or `unsolved`, come from the outcome.
pub fn states(
    sketch: &Sketch,
    diagnosis: &Diagnosis,
    outcome: Result<&SketchSolution, &Error>,
) -> SketchStates {
    let _span = tracing::info_span!("sketch states").entered();
    if is_empty(sketch) {
        let (state, dofs) = EMPTY;
        return SketchStates {
            state,
            dofs,
            points: BTreeMap::new(),
            lines: BTreeMap::new(),
            circles: BTreeMap::new(),
            arcs: BTreeMap::new(),
            constraints: BTreeMap::new(),
            solve_problem: None,
        };
    }
    let solved = outcome.is_ok();
    let constraints: BTreeMap<ElementId, ConstraintState> = sketch
        .constraints
        .iter()
        .map(|c| (c.id.clone(), constraint_state(c, diagnosis, solved)))
        .collect();
    // What the flagged constraints govern: the points their rows read, and the curves they
    // name.
    let mut points_over: BTreeSet<&ElementId> = BTreeSet::new();
    let mut named: BTreeSet<&ElementId> = BTreeSet::new();
    for c in &sketch.constraints {
        if !constraints.get(&c.id).is_some_and(|s| s.flagged()) {
            continue;
        }
        if let Some(read) = diagnosis.points_read.get(&c.id) {
            points_over.extend(read);
        }
        named.extend(
            c.form
                .references()
                .into_iter()
                .filter_map(|(_, r, _)| r.element()),
        );
    }
    let free = |id: &ElementId| diagnosis.free.contains(id);
    let points = sketch
        .points
        .iter()
        .map(|p| {
            let state = point_state(points_over.contains(&p.id), free(&p.id));
            (p.id.clone(), state)
        })
        .collect();
    // A curve's values in all: its own (a circle's radius, an arc's radius and angles, none
    // for a line), and its points'.
    let curve = |id: &ElementId, own_free: bool, ends: &[&ElementId]| {
        let point_free = ends.iter().any(|&p| free(p));
        let point_over = ends.iter().any(|p| points_over.contains(p));
        let state = curve_state(named.contains(id), point_over, own_free || point_free);
        (id.clone(), state)
    };
    let lines = sketch
        .lines
        .iter()
        .map(|l| curve(&l.id, false, &[&l.from, &l.to]))
        .collect();
    let circles = sketch
        .circles
        .iter()
        .map(|c| curve(&c.id, free(&c.id), &[&c.center]))
        .collect();
    let arcs = sketch
        .arcs
        .iter()
        .map(|a| curve(&a.id, free(&a.id), &[&a.center, &a.start, &a.end]))
        .collect();
    let repeats = constraints.values().any(|s| {
        matches!(
            s,
            ConstraintState::Redundant | ConstraintState::PartiallyRedundant
        )
    });
    SketchStates {
        state: sketch_state(outcome, repeats, diagnosis.dofs),
        dofs: diagnosis.dofs,
        points,
        lines,
        circles,
        arcs,
        constraints,
        solve_problem: outcome.err().cloned(),
    }
}

/// Whether `sketch` is empty: nothing in it to define.
fn is_empty(sketch: &Sketch) -> bool {
    sketch.points.is_empty()
        && sketch.lines.is_empty()
        && sketch.circles.is_empty()
        && sketch.arcs.is_empty()
        && sketch.constraints.is_empty()
}

/// The sketch's state: a function of the solve's problem's code, then the flagged lists,
/// then the degrees of freedom, in that order (the decision's table). The code is the one
/// the core's read returns, so no second order can drift from the read's.
fn sketch_state(outcome: Result<&SketchSolution, &Error>, repeats: bool, dofs: i32) -> SketchState {
    match outcome {
        Err(e) => match e.code() {
            ErrorCode::OverConstrained => conflict_state(),
            ErrorCode::SolveFailed => SketchState::NoSolution,
            ErrorCode::InvalidResult => SketchState::Invalid,
            // Not in the decision's table (docs/ports/sketcher.md, *Built*): a solution a
            // load can't translate again (an arc whose ends it puts at one angle from its
            // centre) is geometry that can't be held; a reply the core can't read, a
            // worker's or a bug's, no solution it stands by.
            ErrorCode::InvalidInput => SketchState::Invalid,
            _ => SketchState::NoSolution,
        },
        Ok(_) if repeats => redundancy_state(),
        Ok(_) if dofs > 0 => SketchState::UnderDefined,
        Ok(_) => SketchState::FullyDefined,
    }
}

/// The decision's 1: a sketch whose constraints conflict (PlaneGCS's, or decision 5's
/// disagreeing redundancy, both `over_constrained`) is `over_defined`, SolidWorks' meaning,
/// whose over-defined text is the conflict text; `no_solution` is for a sketch the solver
/// couldn't place with no constraint in excess.
fn conflict_state() -> SketchState {
    SketchState::OverDefined
}

/// The decision's 2: a sketch that solves with constraints that repeat others and agree,
/// kept, is `over_defined`, though its features build: SolidWorks' over-defining prompt is
/// for exactly this.
fn redundancy_state() -> SketchState {
    SketchState::OverDefined
}

/// A constraint's state, on a sketch `solved` or refused: `conflicting` if it's in a
/// conflict; otherwise `redundant` or `partially_redundant` as PlaneGCS lists it (one it
/// lists as both is redundant, as the read keeps it); otherwise `satisfied` on a solved
/// sketch and `unsolved` on a refused one (the decision's A). A reference dimension
/// (`driving: false`) is never sent to the solver, so it's never flagged and fixes nothing:
/// `satisfied` on a solved sketch, `unsolved` otherwise.
fn constraint_state(c: &SketchConstraint, diagnosis: &Diagnosis, solved: bool) -> ConstraintState {
    let sent = c.form.driving() != Some(false);
    if sent && diagnosis.conflicting.contains(&c.id) {
        ConstraintState::Conflicting
    } else if sent && diagnosis.redundant.contains(&c.id) {
        ConstraintState::Redundant
    } else if sent && diagnosis.partially_redundant.contains(&c.id) {
        ConstraintState::PartiallyRedundant
    } else if solved {
        ConstraintState::Satisfied
    } else {
        ConstraintState::Unsolved
    }
}

/// A point's state (the decision's 3): `over_defined` if a flagged constraint's rows read
/// its x or y ([`SketchSystem::reads`](silico_protocol::SketchSystem::reads)), before its
/// freedom; otherwise `under_defined` if its x or y is free; otherwise `fully_defined`. A
/// point no constraint reads is free (a zero column of the diagnosis), so it's
/// `under_defined` with no special case; a point curves share has one state, whichever
/// curves use it. So a point on two curves that touch at it, one `on` redundant at first
/// order, is `over_defined`, its unusable freedom left in `dofs`.
fn point_state(read_by_flagged: bool, free: bool) -> MemberState {
    if read_by_flagged {
        MemberState::OverDefined
    } else if free {
        MemberState::UnderDefined
    } else {
        MemberState::FullyDefined
    }
}

/// A curve's state: `over_defined` if it's over-defined ([`curve_over_defined`]);
/// otherwise `under_defined` if any of its values in all, its own and its points', is free
/// (FreeCAD's `getGeometry`: a line fully constrained when both its ends are, a circle when
/// its centre and radius are, an arc when its centre, ends, radius, and angles are);
/// otherwise `fully_defined`.
fn curve_state(named_by_flagged: bool, point_over: bool, free: bool) -> MemberState {
    if curve_over_defined(named_by_flagged, point_over) {
        MemberState::OverDefined
    } else if free {
        MemberState::UnderDefined
    } else {
        MemberState::FullyDefined
    }
}

/// The decision's 3: a curve is over-defined when a flagged constraint names it, not when a
/// point of it is over-defined: it takes its points' freedom but not their over-definition.
/// So two lines through a point dimensioned twice aren't over defined; the point is, and so
/// is the repeated dimension. The alternative, `named_by_flagged || point_over`, would
/// redden every line through a twice-dimensioned corner and point at nothing removable.
fn curve_over_defined(named_by_flagged: bool, point_over: bool) -> bool {
    let _ = point_over;
    named_by_flagged
}
