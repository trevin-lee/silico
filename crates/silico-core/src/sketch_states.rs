//! Each sketch member's definition state, as SolidWorks names them, from the solve's
//! diagnosis, as M5.0 open item 32 decided them, 2026-10-29 (docs/ports/sketcher.md,
//! *Definition states (the sketch read, M5.10, open item 32)*): the sketch's
//! `under_defined`, `fully_defined`, `over_defined`, `no_solution`, or `invalid`; each
//! point's and curve's `under_defined`, `fully_defined`, or `over_defined`; each constraint's
//! `satisfied`, `conflicting`, `redundant`, `partially_redundant`, or `unsolved`; and the
//! solve's problem, the core's error, for a sketch whose solve was refused. The core decides
//! them, so no client maps unknowns to members.
//!
//! Every rule is decided here, each in a function of its own, so any one can change: the
//! sketch's state from the refusal's code, then the over-defining constraints, then the
//! degrees of freedom ([`refusal_state`], [`sketch_state`]); which constraints over-define
//! ([`over_defines`]: a conflict, or a repeated driving dimension, never a repeated relation,
//! as SolidWorks' FAQ has it); a point's ([`point_state`]) and a curve's
//! ([`curve_over_defined`], [`curve_state`]); a constraint's, `unsolved` on a refused sketch
//! ([`constraint_state`]); and the empty sketch's ([`EMPTY_STATE`], [`EMPTY_DOFS`]). Which
//! constraint of a dependent group is flagged, and as what, is PlaneGCS's choice as it lists
//! it (its popularity contest), never a rule here: the symmetric arc's case is the solver's
//! answer, reported as it gives it.

use std::collections::{BTreeMap, BTreeSet};

use crate::document::{ElementId, Form, Sketch, SketchConstraint};
use crate::sketch::{Diagnosed, Diagnosis};
use crate::{Error, ErrorCode};

/// A sketch's state: SolidWorks' five (docs/ux/solidworks.md, §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SketchState {
    /// Solved, with degrees of freedom left, and no driving dimension repeating others (a
    /// repeated relation may be).
    UnderDefined,
    /// Solved, with none left and no driving dimension repeating others (a repeated relation
    /// may be); or empty.
    FullyDefined,
    /// Constraints that conflict (refused), or a driving dimension that repeats others and
    /// agrees (solved).
    OverDefined,
    /// No algorithm placed it, or its loads don't settle.
    NoSolution,
    /// Solved only to geometry that can't be built, or that breaks what its constraints
    /// mean.
    Invalid,
}

impl SketchState {
    /// Every state, in SolidWorks' order (docs/ux/solidworks.md, §2).
    pub const ALL: [SketchState; 5] = [
        SketchState::UnderDefined,
        SketchState::FullyDefined,
        SketchState::OverDefined,
        SketchState::NoSolution,
        SketchState::Invalid,
    ];

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
    /// An over-defining constraint (in conflict, or a driving dimension repeating others)
    /// governs it.
    OverDefined,
}

impl MemberState {
    /// Every state, in SolidWorks' order.
    pub const ALL: [MemberState; 3] = [
        MemberState::UnderDefined,
        MemberState::FullyDefined,
        MemberState::OverDefined,
    ];

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
    /// Every state, satisfied first, then those flagged, then unsolved.
    pub const ALL: [ConstraintState; 5] = [
        ConstraintState::Satisfied,
        ConstraintState::Conflicting,
        ConstraintState::Redundant,
        ConstraintState::PartiallyRedundant,
        ConstraintState::Unsolved,
    ];

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

    /// Whether the constraint is flagged: listed by the diagnosis, in conflict or repeating
    /// others.
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
    /// Degrees of freedom left, PlaneGCS's number as given: its unknowns less the rank of
    /// its equations, so 0 or more while something is free, a conflict or not (two lengths
    /// on one free line leave 3). Where nothing is free and a linear dependency among its
    /// equations holds no constraint PlaneGCS found redundant (a conflict, or an arc's own
    /// rules repeating each other), it counts the equations left in place of their rank, and
    /// the number is negative: a 40 by 20 rectangle given a second width of 50 reads -1. 0
    /// for an empty sketch.
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

/// An empty sketch's state (no points, so no curves or constraints): `fully_defined`, as
/// FreeCAD's "Empty sketch" is fully constrained with DoF 0 (1.1.3; M5.0 open item 32, the
/// empty sketch).
pub const EMPTY_STATE: SketchState = SketchState::FullyDefined;

/// An empty sketch's degrees of freedom: none. PlaneGCS's −1 for no unknowns is never read.
pub const EMPTY_DOFS: i32 = 0;

/// The states of `sketch`, from `read`, its solve's diagnosis and outcome, read together from
/// the answer that stands ([`Answer::diagnosed`](crate::sketch::Answer::diagnosed)), or a
/// refused translation's ([`Diagnosed::untranslated`]). Points' and curves' states come from
/// the diagnosis whatever the outcome, so a refused sketch's members (given as stored) have
/// them too; the sketch's state, and whether a constraint not flagged is `satisfied` or
/// `unsolved`, come from the outcome.
///
/// An outcome whose error is no sketch state (a worker's, a reply the core can't read, a
/// bug's: any code but `over_constrained`, `solve_failed`, `invalid_result`, and
/// `invalid_input`) has no states: it's the error, so a read of the sketch fails with it
/// rather than telling an agent to move the geometry when the fault is elsewhere.
pub fn states(sketch: &Sketch, read: &Diagnosed) -> Result<SketchStates, Error> {
    let _span = tracing::info_span!("sketch states").entered();
    let (diagnosis, outcome) = (read.diagnosis(), read.outcome());
    let refused = match outcome {
        Ok(_) => None,
        Err(e) => match refusal_state(e.code()) {
            Some(state) => Some(state),
            None => return Err(e.clone()),
        },
    };
    if is_empty(sketch) {
        return Ok(SketchStates {
            state: EMPTY_STATE,
            dofs: EMPTY_DOFS,
            points: BTreeMap::new(),
            lines: BTreeMap::new(),
            circles: BTreeMap::new(),
            arcs: BTreeMap::new(),
            constraints: BTreeMap::new(),
            solve_problem: None,
        });
    }
    let solved = outcome.is_ok();
    let constraints: BTreeMap<ElementId, ConstraintState> = sketch
        .constraints
        .iter()
        .map(|c| (c.id.clone(), constraint_state(c, diagnosis, solved)))
        .collect();
    // What the over-defining constraints govern: the points their equations depend on, and
    // the curves they name.
    let over = |c: &SketchConstraint| {
        constraints
            .get(&c.id)
            .is_some_and(|&s| over_defines(s, &c.form))
    };
    let mut points_over: BTreeSet<&ElementId> = BTreeSet::new();
    let mut named: BTreeSet<&ElementId> = BTreeSet::new();
    for c in sketch.constraints.iter().filter(|c| over(c)) {
        if let Some(depended_on) = diagnosis.points_depended_on.get(&c.id) {
            points_over.extend(depended_on);
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
        let state = curve_state(named.contains(id), own_free || point_free);
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
    let over_defining = sketch.constraints.iter().any(over);
    Ok(SketchStates {
        state: sketch_state(refused, over_defining, diagnosis.dofs),
        dofs: diagnosis.dofs,
        points,
        lines,
        circles,
        arcs,
        constraints,
        solve_problem: outcome.err().cloned(),
    })
}

/// Whether `sketch` is empty: nothing in it to define.
fn is_empty(sketch: &Sketch) -> bool {
    sketch.points.is_empty()
        && sketch.lines.is_empty()
        && sketch.circles.is_empty()
        && sketch.arcs.is_empty()
        && sketch.constraints.is_empty()
}

/// The state of a sketch whose solve was refused with `code`, or none if `code` is no
/// sketch state (M5.0 open item 32's table, and its codes outside the table):
/// - `over_constrained`, PlaneGCS's conflict or decision 5's disagreeing redundancy:
///   `over_defined`, SolidWorks' meaning, whose over-defined text is the conflict text;
///   `no_solution` is for a sketch the solver couldn't place with no constraint in excess;
/// - `solve_failed`: `no_solution`;
/// - `invalid_result`: `invalid`, a solution that can't be built or held, or a solution a
///   load can't translate again;
/// - `invalid_input`: `invalid`, a sketch the translation refuses before any solve (two
///   points drawn at one place where a constraint is measured by the line between them, an
///   arc that sweeps nothing);
/// - any other code, a worker's (`worker_crashed`, `worker_timeout`, `worker_unavailable`), a
///   reply the core can't read (`protocol_error`), or a bug's (`internal`): none. It says
///   nothing about the sketch.
///
/// The code is the one the core's read returns, so no second order can drift from the
/// read's; the read itself sends on, as its error, every code this gives none for.
pub(crate) fn refusal_state(code: ErrorCode) -> Option<SketchState> {
    match code {
        ErrorCode::OverConstrained => Some(SketchState::OverDefined),
        ErrorCode::SolveFailed => Some(SketchState::NoSolution),
        ErrorCode::InvalidResult | ErrorCode::InvalidInput => Some(SketchState::Invalid),
        _ => None,
    }
}

/// The sketch's state: the refusal's, if the solve was `refused`; then, solved, any
/// constraint that over-defines ([`over_defines`]), then the degrees of freedom. Solved, an
/// over-defining constraint makes it over defined (SolidWorks' over-defining prompt is for
/// exactly this: a repeated dimension that agrees, kept, though its features build), so
/// `fully_defined` never has a conflict or a repeated driving dimension; a repeated relation
/// leaves it as its degrees of freedom say (SolidWorks' FAQ: "Redundant perpendicular
/// relation added. Sketch is still fully defined.").
fn sketch_state(refused: Option<SketchState>, over_defining: bool, dofs: i32) -> SketchState {
    match refused {
        Some(state) => state,
        None if over_defining => SketchState::OverDefined,
        None if dofs > 0 => SketchState::UnderDefined,
        None => SketchState::FullyDefined,
    }
}

/// A constraint's state, on a sketch `solved` or refused: `conflicting` if it's in a
/// conflict; otherwise `redundant` or `partially_redundant` as PlaneGCS lists it (one it
/// lists as both is redundant, as the read keeps it); otherwise `satisfied` on a solved
/// sketch and `unsolved` on a refused one (item 32's fifth constraint state: SolidWorks' "Not
/// Solved", so `satisfied` always means met on the coordinates the read shows). A reference
/// dimension (`driving: false`) is never sent to the solver, so it's never flagged and fixes
/// nothing: `satisfied` on a solved sketch, `unsolved` otherwise.
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

/// Whether a constraint of `form`, in `state`, over-defines what it governs
/// (docs/ports/sketcher.md, *Definition states*, *An over-defining constraint*, from
/// SolidWorks' FAQ on sketch relations): when it's `conflicting`, or when it's `redundant`
/// or `partially_redundant` and a dimension ([`Form::is_dimension`]: `length`, `distance`,
/// `distance_x`, `distance_y`, `point_line_distance`, `angle`, `radius`, `diameter`). A
/// repeated relation (the other 13 forms) over-defines nothing: it has no value of its own
/// to edit, so only an edit elsewhere can break it, and that edit's solve reports the
/// conflict; a dimension's own value can. A reference dimension is never flagged
/// ([`constraint_state`]), so never over-defines.
///
/// Which constraint of a dependent group is flagged is PlaneGCS's choice, blind to the
/// kinds: a group of a relation and a dimension, its relation flagged, over-defines nothing
/// (the one divergence from SolidWorks this rule leaves; docs/ports/sketcher.md).
fn over_defines(state: ConstraintState, form: &Form) -> bool {
    state == ConstraintState::Conflicting || (state.flagged() && form.is_dimension())
}

/// A point's state (item 32's members' rule): `over_defined` if an over-defining
/// constraint's equations depend on its x or y
/// ([`SketchSystem::depends_on`](silico_protocol::SketchSystem::depends_on)), before its
/// freedom; otherwise `under_defined` if its x or y is free; otherwise `fully_defined`. A
/// point no constraint reads is free (a zero column of the diagnosis), so it's
/// `under_defined` with no special case; a point curves share has one state, whichever
/// curves use it. So a point on two curves that touch at it, one `on` redundant at first
/// order, a relation, keeps the state its freedom gives (`under_defined`, the degree it
/// can't use left in `dofs`); and an arc's far end, which a conflicting tangency at its other
/// end never depends on, keeps the state its freedom gives.
fn point_state(depended_on_by_over_defining: bool, free: bool) -> MemberState {
    if depended_on_by_over_defining {
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
fn curve_state(named_by_over_defining: bool, free: bool) -> MemberState {
    if curve_over_defined(named_by_over_defining) {
        MemberState::OverDefined
    } else if free {
        MemberState::UnderDefined
    } else {
        MemberState::FullyDefined
    }
}

/// Item 32's members' rule: a curve is over-defined when an over-defining constraint names
/// it, not when a point of it is over-defined: it takes its points' freedom but not their
/// over-definition. So two lines through a point dimensioned twice aren't over defined; the
/// point is, and so is the repeated dimension. The alternative, over-defined whenever a point
/// of it is, would redden every line through a twice-dimensioned corner and point at nothing
/// removable.
///
/// It's the rule's hook, so the rule can change here alone: today an over-defining
/// constraint's naming the curve decides it, so it gives that back as it's given.
fn curve_over_defined(named_by_over_defining: bool) -> bool {
    named_by_over_defining
}
