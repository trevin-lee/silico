//! A sketch as PlaneGCS sees it (docs/ports/sketcher.md): values, which of them the solver
//! may change, geometry made of those values, and constraints on the geometry, each with
//! the tag the solver reports it by. The core builds this from a document's sketch; the
//! sketch worker solves it and says what it found ([`Solved`]).
//!
//! Everything is in the solver's terms: lengths in the document's unit, angles in radians,
//! and the values in one list whose order is the core's, so the worker can hand PlaneGCS
//! the values in that order (its order of unknowns is their order in memory).

use serde::{Deserialize, Serialize};

use crate::primitive::{InvalidParameter, MAX_LENGTH, format_number};

/// At most this many values in a system.
pub const MAX_SKETCH_VALUES: usize = 10_000;

/// At most this many constraints in a system.
pub const MAX_SKETCH_CONSTRAINTS: usize = 10_000;

/// At most this many pieces of geometry in a system.
pub const MAX_SKETCH_GEOMETRY: usize = 10_000;

/// At most this many unknowns (about 500 lines): Levenberg–Marquardt forms a dense
/// unknowns × unknowns matrix (32 MiB at this limit) and factors it in time that grows as
/// the cube: about 13 s for 2,000 unknowns, 100 s for 4,000.
pub const MAX_SKETCH_UNKNOWNS: usize = 2048;

/// At most this many entries in the solver's Jacobian (rows × unknowns): PlaneGCS's
/// diagnosis builds it dense, so this bounds it to 256 MiB.
pub const MAX_JACOBIAN_ENTRIES: usize = 1 << 25;

/// The largest tag: PlaneGCS takes tags as C `int`s, and leaves a negative one out of its
/// diagnosis.
pub const MAX_TAG: u32 = i32::MAX as u32;

/// A value of the system, by its place in [`SketchSystem::values`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Value(pub u32);

/// A point, by its two coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointValues {
    pub x: Value,
    pub y: Value,
}

/// A piece of geometry, by the values it's made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SketchGeometry {
    Point(PointValues),
    /// A line segment from `start` to `end`.
    Line {
        start: PointValues,
        end: PointValues,
    },
    Circle {
        center: PointValues,
        radius: Value,
    },
    /// An arc of a circle, counter-clockwise from `start_angle` to `end_angle` (radians),
    /// with its end points: [`SolverConstraint::ArcRules`] ties them to the angles.
    Arc {
        center: PointValues,
        radius: Value,
        start_angle: Value,
        end_angle: Value,
        start: PointValues,
        end: PointValues,
    },
}

impl SketchGeometry {
    fn kind(&self) -> GeometryKind {
        match self {
            SketchGeometry::Point(_) => GeometryKind::Point,
            SketchGeometry::Line { .. } => GeometryKind::Line,
            SketchGeometry::Circle { .. } => GeometryKind::Circle,
            SketchGeometry::Arc { .. } => GeometryKind::Arc,
        }
    }

    /// The values it's made of, in the order the sketch worker hands them to PlaneGCS.
    pub fn values(&self) -> Vec<Value> {
        match *self {
            SketchGeometry::Point(p) => vec![p.x, p.y],
            SketchGeometry::Line { start, end } => vec![start.x, start.y, end.x, end.y],
            SketchGeometry::Circle { center, radius } => vec![center.x, center.y, radius],
            SketchGeometry::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                start,
                end,
            } => vec![
                center.x,
                center.y,
                radius,
                start_angle,
                end_angle,
                start.x,
                start.y,
                end.x,
                end.y,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeometryKind {
    Point,
    Line,
    Circle,
    Arc,
}

impl GeometryKind {
    fn word(self) -> &'static str {
        match self {
            GeometryKind::Point => "a point",
            GeometryKind::Line => "a line",
            GeometryKind::Circle => "a circle",
            GeometryKind::Arc => "an arc",
        }
    }
}

/// A piece of geometry, by its place in [`SketchSystem::geometry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Geometry(pub u32);

/// A point of a piece of geometry: a point itself, a line's or arc's ends, a circle's or
/// arc's centre (FreeCAD's `PointPos`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointOf {
    Point(Geometry),
    Start(Geometry),
    End(Geometry),
    Center(Geometry),
}

impl PointOf {
    fn geometry(self) -> Geometry {
        match self {
            PointOf::Point(g) | PointOf::Start(g) | PointOf::End(g) | PointOf::Center(g) => g,
        }
    }
}

impl std::fmt::Display for PointOf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PointOf::Point(g) => write!(f, "point {}", g.0),
            PointOf::Start(g) => write!(f, "the start of geometry {}", g.0),
            PointOf::End(g) => write!(f, "the end of geometry {}", g.0),
            PointOf::Center(g) => write!(f, "the centre of geometry {}", g.0),
        }
    }
}

/// A constraint, in PlaneGCS's terms. Each named value is a fixed [`Value`] of the system:
/// reference dimensions, whose value the solver finds, come with the document's sketches
/// (M3.3). Every one except [`ArcRules`](SolverConstraint::ArcRules) carries the tag the
/// solver reports it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverConstraint {
    /// An arc's end points on its circle at its angles: implicit in every arc whose values
    /// the solver may change, never reported (PlaneGCS's tag 0). Each of its four
    /// equations must involve an unknown: on fixed values they're rows of zeros that count
    /// against the degrees of freedom, as FreeCAD found (`c6de527391`: fixed arcs get no
    /// rules).
    ArcRules {
        arc: Geometry,
    },
    /// Two points in the same place.
    Coincident {
        tag: Tag,
        a: PointOf,
        b: PointOf,
    },
    /// A line parallel to the x axis.
    Horizontal {
        tag: Tag,
        line: Geometry,
    },
    /// A line parallel to the y axis.
    Vertical {
        tag: Tag,
        line: Geometry,
    },
    /// `value` = x(b) − x(a).
    DistanceX {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        value: Value,
    },
    /// `value` = y(b) − y(a).
    DistanceY {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        value: Value,
    },
    /// The distance between two points.
    Distance {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        value: Value,
    },
    /// A point on a line (extended past its ends).
    PointOnLine {
        tag: Tag,
        point: PointOf,
        line: Geometry,
    },
    Parallel {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    Perpendicular {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    /// The angle from line `a`'s direction to line `b`'s, counter-clockwise (radians).
    Angle {
        tag: Tag,
        a: Geometry,
        b: Geometry,
        value: Value,
    },
    /// Two lines of the same length.
    EqualLength {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    /// A circle's or arc's radius.
    Radius {
        tag: Tag,
        circle: Geometry,
        value: Value,
    },
}

/// The number a constraint is reported by: from 1 (PlaneGCS never reports tag 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Tag(pub u32);

/// How PlaneGCS solves (its `Algorithm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Algorithm {
    DogLeg,
    LevenbergMarquardt,
    Bfgs,
}

/// What the sketch worker solves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchSystem {
    /// Every value, in the core's order: the solver starts from these.
    pub values: Vec<f64>,
    /// The values the solver may change, in increasing order. The rest are fixed.
    pub unknowns: Vec<Value>,
    pub geometry: Vec<SketchGeometry>,
    pub constraints: Vec<SolverConstraint>,
    /// The algorithms to try, in order, after one diagnosis, until one solves (FreeCAD's
    /// fallback: DogLeg, then Levenberg–Marquardt, then BFGS; its last step, SQP on a system
    /// anchored at the given values, comes with M3.3): one to three, each once.
    pub algorithms: Vec<Algorithm>,
}

/// How a solve ended: PlaneGCS's `SolveStatus`, and one reading of it. A status means
/// something only when the diagnosis found no conflict: FreeCAD doesn't solve a conflicting
/// sketch, and PlaneGCS can report `Success` for one with its conflicting constraints unmet.
/// And PlaneGCS solves only the parts of a sketch with unknowns: a constraint among fixed
/// values alone isn't solved, whatever the status, and its residual says whether it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolveStatus {
    /// Every constraint the solver solved is met, and none was dropped as redundant.
    Success,
    /// Equations were dropped as redundant (`redundant`, `partially_redundant`), every
    /// constraint the solver kept is met, and the dropped ones weren't checked on the
    /// solution: their residuals say whether they agree. PlaneGCS checks them on the values
    /// it started from, before writing its solution back, and calls the solve `Converged`
    /// when they don't hold there, so its own status depends on where the sketch was drawn;
    /// this one doesn't. From DogLeg and Levenberg–Marquardt, which otherwise end only in
    /// success or failure, that's all a `Converged` can mean. Whether the dropped equations
    /// agree enough is the core's (docs/roadmap/m3-sketcher.md, decision 5); FreeCAD fails
    /// such a sketch unless it started where they hold.
    SolvedExceptRedundant,
    /// The solver stopped at a minimum where the constraints aren't all met (BFGS), or found
    /// the redundant equations unmet on the values it started from (BFGS too: its own
    /// `Converged` and that one can't be told apart, so, as FreeCAD does, it's unsolved).
    Converged,
    /// The solver failed.
    Failed,
}

/// What the sketch worker found: the diagnosis, then the solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solved {
    /// Degrees of freedom left: negative when over-constrained.
    pub dofs: i32,
    /// Constraints in conflict, by tag, in increasing order.
    pub conflicting: Vec<Tag>,
    /// Constraints that repeat others, by tag, in increasing order.
    pub redundant: Vec<Tag>,
    /// Constraints some of whose equations repeat others, by tag, in increasing order.
    pub partially_redundant: Vec<Tag>,
    /// Unknowns PlaneGCS finds the constraints don't fix, each once, in increasing order.
    /// It finds none when there are no equations to diagnose (`no_equations`), which
    /// FreeCAD reads as every unknown free.
    pub free: Vec<Value>,
    /// The diagnosis had no equations: no constraint touches an unknown.
    pub no_equations: bool,
    /// How the solve ended: the first algorithm to solve (`Success` or
    /// `SolvedExceptRedundant`), or the last tried.
    pub status: SolveStatus,
    /// The algorithm that solved, exactly when the status is `Success` or
    /// `SolvedExceptRedundant`.
    pub algorithm: Option<Algorithm>,
    /// Every value after the solve: the solution when the status is `Success` or
    /// `SolvedExceptRedundant`, otherwise as given.
    pub values: Vec<f64>,
    /// Each tagged constraint's residual on those values, by tag in increasing order, as
    /// PlaneGCS measures it: its error for a constraint of one equation, the root mean
    /// square of them for one of several, in the constraint's own terms: millimetres for
    /// distances and coincidences, radians for angles, and for parallel and perpendicular a
    /// sine or cosine scaled by the lines' starting lengths over their solved ones
    /// (PlaneGCS fixes the scale when it builds the constraint). `None` where it isn't a
    /// number (geometry of no size).
    pub residuals: Vec<(Tag, Option<f64>)>,
}

impl SketchSystem {
    /// Whether the worker can solve this: sizes within the limits, every value finite and
    /// within reach, every reference in range and of the right kind, unknowns in
    /// increasing order, tags from 1, each used once, and an arc's rules only for an arc.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        let n = self.values.len();
        for (parameter, count, limit, what) in [
            ("values", n, MAX_SKETCH_VALUES, "values"),
            (
                "unknowns",
                self.unknowns.len(),
                MAX_SKETCH_UNKNOWNS,
                "unknowns",
            ),
            (
                "geometry",
                self.geometry.len(),
                MAX_SKETCH_GEOMETRY,
                "pieces of geometry",
            ),
            (
                "constraints",
                self.constraints.len(),
                MAX_SKETCH_CONSTRAINTS,
                "constraints",
            ),
        ] {
            if count > limit {
                return Err(invalid(
                    parameter,
                    format!("a sketch may have at most {limit} {what}, got {count}"),
                ));
            }
        }
        let algorithms = &self.algorithms;
        if algorithms.is_empty()
            || algorithms.len() > 3
            || (1..algorithms.len()).any(|i| algorithms[..i].contains(&algorithms[i]))
        {
            return Err(invalid(
                "algorithms",
                format!("algorithms must be one to three different ones, got {algorithms:?}"),
            ));
        }
        // A kilometre, or as many radians: everything the kernel can make is within reach.
        if let Some((i, v)) = self
            .values
            .iter()
            .enumerate()
            .find(|(_, v)| !within_reach(**v))
        {
            return Err(invalid(
                "values",
                format!(
                    "value {i} must be finite and within {} of 0, got {}",
                    format_number(MAX_LENGTH),
                    format_number(*v)
                ),
            ));
        }
        let value = |v: Value, what: &str| -> Result<(), InvalidParameter> {
            if (v.0 as usize) < n {
                Ok(())
            } else {
                Err(invalid(
                    "values",
                    format!("{what} names value {}, but there are {n}", v.0),
                ))
            }
        };
        for pair in self.unknowns.windows(2) {
            if pair[0] >= pair[1] {
                return Err(invalid(
                    "unknowns",
                    format!(
                        "unknowns must be in increasing order, each once: {} comes before {}",
                        pair[0].0, pair[1].0
                    ),
                ));
            }
        }
        for &u in &self.unknowns {
            value(u, "an unknown")?;
        }
        for (i, g) in self.geometry.iter().enumerate() {
            for v in g.values() {
                value(v, &format!("geometry {i}"))?;
            }
            // A line drawn with no length gives the solver 0/0 for its direction: wrong
            // diagnoses, not errors. FreeCAD can't draw one. The solver works the length out
            // as the square root of dx² + dy², so ends close enough for that to be 0 (1e-200
            // apart, say) are no length to it either.
            if let SketchGeometry::Line { start, end } = *g {
                let at = |v: Value| self.values[v.0 as usize];
                let (dx, dy) = (at(end.x) - at(start.x), at(end.y) - at(start.y));
                if dx * dx + dy * dy == 0.0 {
                    return Err(invalid(
                        "geometry",
                        format!(
                            "geometry {i} is a line drawn with no length, its ends at ({}, {}) and ({}, {})",
                            format_number(at(start.x)),
                            format_number(at(start.y)),
                            format_number(at(end.x)),
                            format_number(at(end.y))
                        ),
                    ));
                }
            }
        }
        let mut tags = Vec::with_capacity(self.constraints.len());
        let mut ruled = Vec::new();
        let mut rows = 0usize;
        for (i, c) in self.constraints.iter().enumerate() {
            self.check_constraint(c).map_err(|e| InvalidParameter {
                parameter: e.parameter,
                message: match c.tag() {
                    Some(tag) => format!("constraint {i} (tag {}): {}", tag.0, e.message),
                    None => format!("constraint {i}: {}", e.message),
                },
            })?;
            match c.tag() {
                Some(tag) => tags.push(tag),
                None => {
                    if let SolverConstraint::ArcRules { arc } = *c {
                        ruled.push(arc);
                    }
                }
            }
            rows += c.rows();
        }
        ruled.sort_unstable();
        if let Some(pair) = ruled.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(invalid(
                "constraints",
                format!("arc {}'s rules are given twice", pair[0].0),
            ));
        }
        tags.sort_unstable();
        if let Some(pair) = tags.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(invalid(
                "constraints",
                format!(
                    "tag {} is used twice: each constraint has its own",
                    pair[0].0
                ),
            ));
        }
        if rows.saturating_mul(self.unknowns.len()) > MAX_JACOBIAN_ENTRIES {
            return Err(invalid(
                "constraints",
                format!(
                    "{rows} equations in {} unknowns is more than the solver can diagnose (at most {MAX_JACOBIAN_ENTRIES} entries)",
                    self.unknowns.len()
                ),
            ));
        }
        Ok(())
    }

    /// One constraint's references, values, and tag: the parts of [`check`](Self::check)
    /// a refusal names the constraint for.
    fn check_constraint(&self, c: &SolverConstraint) -> Result<(), InvalidParameter> {
        let n = self.values.len();
        let what = constraint_word(c);
        let kind = |g: Geometry| -> Result<GeometryKind, InvalidParameter> {
            self.geometry
                .get(g.0 as usize)
                .map(SketchGeometry::kind)
                .ok_or_else(|| {
                    invalid(
                        "constraints",
                        format!(
                            "{what} names geometry {}, but there are {}",
                            g.0,
                            self.geometry.len()
                        ),
                    )
                })
        };
        let want = |g: Geometry, kinds: &[GeometryKind]| {
            let k = kind(g)?;
            if kinds.contains(&k) {
                Ok(())
            } else {
                Err(invalid(
                    "constraints",
                    format!(
                        "{what} needs {}, but geometry {} is {}",
                        kinds_word(kinds),
                        g.0,
                        k.word()
                    ),
                ))
            }
        };
        let point = |p: PointOf| -> Result<(), InvalidParameter> {
            let k = kind(p.geometry())?;
            let fits = match p {
                PointOf::Point(_) => k == GeometryKind::Point,
                PointOf::Start(_) | PointOf::End(_) => {
                    matches!(k, GeometryKind::Line | GeometryKind::Arc)
                }
                PointOf::Center(_) => matches!(k, GeometryKind::Circle | GeometryKind::Arc),
            };
            if fits {
                Ok(())
            } else {
                Err(invalid(
                    "constraints",
                    format!(
                        "{what} names {p}, but geometry {} is {}",
                        p.geometry().0,
                        k.word()
                    ),
                ))
            }
        };
        // A dimension's value is fixed: reference dimensions, whose value the solver finds,
        // come with the document's sketches (M3.3).
        let dimension = |v: Value| -> Result<(), InvalidParameter> {
            if (v.0 as usize) >= n {
                return Err(invalid(
                    "values",
                    format!("{what} names value {}, but there are {n}", v.0),
                ));
            }
            if self.unknowns.binary_search(&v).is_ok() {
                return Err(invalid(
                    "constraints",
                    format!(
                        "{what}'s value {} is an unknown: a dimension's value must be fixed",
                        v.0
                    ),
                ));
            }
            Ok(())
        };
        use GeometryKind::{Arc, Circle, Line};
        match *c {
            SolverConstraint::ArcRules { arc } => {
                want(arc, &[Arc])?;
                self.check_arc_rules(arc)?;
            }
            SolverConstraint::Coincident { a, b, .. } => {
                point(a)?;
                point(b)?;
            }
            SolverConstraint::Horizontal { line, .. } | SolverConstraint::Vertical { line, .. } => {
                want(line, &[Line])?
            }
            SolverConstraint::DistanceX { a, b, value: v, .. }
            | SolverConstraint::DistanceY { a, b, value: v, .. }
            | SolverConstraint::Distance { a, b, value: v, .. } => {
                point(a)?;
                point(b)?;
                dimension(v)?;
            }
            SolverConstraint::PointOnLine { point: p, line, .. } => {
                point(p)?;
                want(line, &[Line])?;
            }
            SolverConstraint::Parallel { a, b, .. }
            | SolverConstraint::Perpendicular { a, b, .. }
            | SolverConstraint::EqualLength { a, b, .. } => {
                want(a, &[Line])?;
                want(b, &[Line])?;
            }
            SolverConstraint::Angle { a, b, value: v, .. } => {
                want(a, &[Line])?;
                want(b, &[Line])?;
                dimension(v)?;
            }
            SolverConstraint::Radius {
                circle, value: v, ..
            } => {
                want(circle, &[Circle, Arc])?;
                dimension(v)?;
            }
        }
        if let Some(tag) = c.tag()
            && !(1..=MAX_TAG).contains(&tag.0)
        {
            return Err(invalid(
                "constraints",
                format!(
                    "{what}'s tag must be from 1 to {MAX_TAG} (the solver never reports tag 0, and takes tags as C ints), got {}",
                    tag.0
                ),
            ));
        }
        Ok(())
    }
}

impl SketchSystem {
    /// An arc's rules are four equations, the start's x and y and the end's from the
    /// centre, radius, and angle: each must involve an unknown (FreeCAD `c6de527391`).
    fn check_arc_rules(&self, arc: Geometry) -> Result<(), InvalidParameter> {
        let Some(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        }) = self.geometry.get(arc.0 as usize).copied()
        else {
            return Ok(());
        };
        let unknown = |v: Value| self.unknowns.binary_search(&v).is_ok();
        for (what, row) in [
            ("start's x", [start.x, center.x, radius, start_angle]),
            ("start's y", [start.y, center.y, radius, start_angle]),
            ("end's x", [end.x, center.x, radius, end_angle]),
            ("end's y", [end.y, center.y, radius, end_angle]),
        ] {
            if !row.into_iter().any(unknown) {
                return Err(invalid(
                    "constraints",
                    format!(
                        "an arc's rules on arc {}, whose {what} equation has no unknown: rules go only on an arc the solver may change",
                        arc.0
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Whether a value is one a sketch may hold: finite, and within a kilometre (or as many
/// radians) of 0. A solution must be too, so the next solve can start from it.
pub fn within_reach(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_LENGTH
}

impl SolverConstraint {
    /// The tag it's reported by: every constraint but an arc's rules has one.
    pub fn tag(&self) -> Option<Tag> {
        match *self {
            SolverConstraint::ArcRules { .. } => None,
            SolverConstraint::Coincident { tag, .. }
            | SolverConstraint::Horizontal { tag, .. }
            | SolverConstraint::Vertical { tag, .. }
            | SolverConstraint::DistanceX { tag, .. }
            | SolverConstraint::DistanceY { tag, .. }
            | SolverConstraint::Distance { tag, .. }
            | SolverConstraint::PointOnLine { tag, .. }
            | SolverConstraint::Parallel { tag, .. }
            | SolverConstraint::Perpendicular { tag, .. }
            | SolverConstraint::Angle { tag, .. }
            | SolverConstraint::EqualLength { tag, .. }
            | SolverConstraint::Radius { tag, .. } => Some(tag),
        }
    }

    /// How many equations PlaneGCS makes of it.
    fn rows(&self) -> usize {
        match self {
            SolverConstraint::ArcRules { .. } => 4,
            SolverConstraint::Coincident { .. } => 2,
            _ => 1,
        }
    }
}

fn constraint_word(c: &SolverConstraint) -> &'static str {
    match c {
        SolverConstraint::ArcRules { .. } => "an arc's rules",
        SolverConstraint::Coincident { .. } => "a coincidence",
        SolverConstraint::Horizontal { .. } => "a horizontal constraint",
        SolverConstraint::Vertical { .. } => "a vertical constraint",
        SolverConstraint::DistanceX { .. } => "a horizontal distance",
        SolverConstraint::DistanceY { .. } => "a vertical distance",
        SolverConstraint::Distance { .. } => "a distance",
        SolverConstraint::PointOnLine { .. } => "a point on a line",
        SolverConstraint::Parallel { .. } => "a parallel constraint",
        SolverConstraint::Perpendicular { .. } => "a perpendicular constraint",
        SolverConstraint::Angle { .. } => "an angle",
        SolverConstraint::EqualLength { .. } => "an equal length",
        SolverConstraint::Radius { .. } => "a radius",
    }
}

fn kinds_word(kinds: &[GeometryKind]) -> String {
    let words: Vec<&str> = kinds.iter().map(|k| k.word()).collect();
    words.join(" or ")
}

fn invalid(parameter: &'static str, message: String) -> InvalidParameter {
    InvalidParameter { parameter, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line from (0, 0) to (10, 0), a circle at (0, 5) of radius 2, and an arc; the
    /// line horizontal, its length 10, the circle's radius 2, and the arc's rules.
    fn system() -> SketchSystem {
        let p = |x: u32, y: u32| PointValues {
            x: Value(x),
            y: Value(y),
        };
        SketchSystem {
            // 0..4 the line, 4..7 the circle, 7..16 the arc, 16 and 17 the dimensions.
            values: vec![
                0.0, 0.0, 10.0, 0.0, 0.0, 5.0, 2.0, 1.0, 1.0, 1.0, 0.0, 1.5, 2.0, 1.0, 1.0, 2.0,
                10.0, 2.0,
            ],
            unknowns: (0..16).map(Value).collect(),
            geometry: vec![
                SketchGeometry::Line {
                    start: p(0, 1),
                    end: p(2, 3),
                },
                SketchGeometry::Circle {
                    center: p(4, 5),
                    radius: Value(6),
                },
                SketchGeometry::Arc {
                    center: p(7, 8),
                    radius: Value(9),
                    start_angle: Value(10),
                    end_angle: Value(11),
                    start: p(12, 13),
                    end: p(14, 15),
                },
            ],
            constraints: vec![
                SolverConstraint::ArcRules { arc: Geometry(2) },
                SolverConstraint::Horizontal {
                    tag: Tag(1),
                    line: Geometry(0),
                },
                SolverConstraint::Distance {
                    tag: Tag(2),
                    a: PointOf::Start(Geometry(0)),
                    b: PointOf::End(Geometry(0)),
                    value: Value(16),
                },
                SolverConstraint::Radius {
                    tag: Tag(3),
                    circle: Geometry(1),
                    value: Value(17),
                },
            ],
            algorithms: vec![Algorithm::DogLeg],
        }
    }

    /// The check's refusal of `spoil`ed [`system`], as (parameter, message).
    fn refused(spoil: impl FnOnce(&mut SketchSystem)) -> (&'static str, String) {
        let mut s = system();
        spoil(&mut s);
        let e = s.check().unwrap_err();
        (e.parameter, e.message)
    }

    #[test]
    fn a_well_formed_system_passes() {
        system().check().unwrap();
    }

    #[test]
    fn values_that_arent_finite_or_are_out_of_reach_are_refused_naming_them() {
        for (bad, shown) in [
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (1.000_000_1e6, "1000000.1"),
        ] {
            let (parameter, message) = refused(|s| s.values[5] = bad);
            assert_eq!(parameter, "values");
            assert_eq!(
                message,
                format!("value 5 must be finite and within 1000000 of 0, got {shown}")
            );
        }
        // A kilometre is within reach.
        let mut s = system();
        s.values[5] = -1e6;
        s.check().unwrap();
    }

    #[test]
    fn unknowns_must_increase_and_name_values_there_are() {
        let (_, message) = refused(|s| s.unknowns.swap(2, 3));
        assert_eq!(
            message,
            "unknowns must be in increasing order, each once: 3 comes before 2"
        );
        let (_, message) = refused(|s| s.unknowns[4] = Value(3));
        assert!(message.contains("each once: 3 comes before 3"), "{message}");
        let (parameter, message) = refused(|s| s.unknowns.push(Value(18)));
        assert_eq!(parameter, "values");
        assert_eq!(message, "an unknown names value 18, but there are 18");
    }

    #[test]
    fn geometry_must_be_made_of_values_there_are() {
        let (_, message) = refused(|s| {
            s.geometry[1] = SketchGeometry::Circle {
                center: PointValues {
                    x: Value(4),
                    y: Value(5),
                },
                radius: Value(40),
            };
        });
        assert_eq!(message, "geometry 1 names value 40, but there are 18");
    }

    #[test]
    fn constraints_must_name_geometry_there_is_of_the_kinds_they_take() {
        let (parameter, message) = refused(|s| {
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(1),
                line: Geometry(3),
            };
        });
        assert_eq!(parameter, "constraints");
        assert_eq!(
            message,
            "constraint 1 (tag 1): a horizontal constraint names geometry 3, but there are 3"
        );
        let (_, message) = refused(|s| {
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(1),
                line: Geometry(1),
            };
        });
        assert_eq!(
            message,
            "constraint 1 (tag 1): a horizontal constraint needs a line, but geometry 1 is a circle"
        );
        let (_, message) = refused(|s| {
            s.constraints[3] = SolverConstraint::Radius {
                tag: Tag(3),
                circle: Geometry(0),
                value: Value(17),
            };
        });
        assert_eq!(
            message,
            "constraint 3 (tag 3): a radius needs a circle or an arc, but geometry 0 is a line"
        );
        let (_, message) =
            refused(|s| s.constraints[0] = SolverConstraint::ArcRules { arc: Geometry(1) });
        assert_eq!(
            message,
            "constraint 0: an arc's rules needs an arc, but geometry 1 is a circle"
        );
    }

    #[test]
    fn points_are_named_in_words() {
        assert_eq!(PointOf::Point(Geometry(3)).to_string(), "point 3");
        assert_eq!(
            PointOf::Start(Geometry(0)).to_string(),
            "the start of geometry 0"
        );
        assert_eq!(
            PointOf::End(Geometry(7)).to_string(),
            "the end of geometry 7"
        );
        assert_eq!(
            PointOf::Center(Geometry(2)).to_string(),
            "the centre of geometry 2"
        );
        let (_, message) = refused(|s| {
            s.constraints[2] = SolverConstraint::Distance {
                tag: Tag(2),
                a: PointOf::Start(Geometry(0)),
                b: PointOf::End(Geometry(1)),
                value: Value(16),
            };
        });
        assert_eq!(
            message,
            "constraint 2 (tag 2): a distance names the end of geometry 1, but geometry 1 is a circle"
        );
    }

    #[test]
    fn points_must_be_points_the_geometry_has() {
        // A circle has a centre but no ends; a line has ends but no centre; a point is
        // only itself.
        for (p, kind) in [
            (PointOf::Start(Geometry(1)), "a circle"),
            (PointOf::End(Geometry(1)), "a circle"),
            (PointOf::Center(Geometry(0)), "a line"),
            (PointOf::Point(Geometry(0)), "a line"),
        ] {
            let (_, message) = refused(|s| {
                s.constraints[2] = SolverConstraint::Distance {
                    tag: Tag(2),
                    a: PointOf::Start(Geometry(0)),
                    b: p,
                    value: Value(16),
                };
            });
            assert_eq!(
                message,
                format!(
                    "constraint 2 (tag 2): a distance names {p}, but geometry {} is {kind}",
                    p.geometry().0
                )
            );
        }
        // The arc has all three.
        for p in [
            PointOf::Start(Geometry(2)),
            PointOf::End(Geometry(2)),
            PointOf::Center(Geometry(2)),
        ] {
            let mut s = system();
            s.constraints[2] = SolverConstraint::Distance {
                tag: Tag(2),
                a: PointOf::Start(Geometry(0)),
                b: p,
                value: Value(16),
            };
            s.check().unwrap();
        }
    }

    #[test]
    fn a_dimensions_value_must_be_a_fixed_value_there_is() {
        let (_, message) = refused(|s| {
            s.constraints[2] = SolverConstraint::Distance {
                tag: Tag(2),
                a: PointOf::Start(Geometry(0)),
                b: PointOf::End(Geometry(0)),
                value: Value(3),
            };
        });
        assert_eq!(
            message,
            "constraint 2 (tag 2): a distance's value 3 is an unknown: a dimension's value must be fixed"
        );
        let (_, message) = refused(|s| {
            s.constraints[3] = SolverConstraint::Radius {
                tag: Tag(3),
                circle: Geometry(1),
                value: Value(18),
            };
        });
        assert_eq!(
            message,
            "constraint 3 (tag 3): a radius names value 18, but there are 18"
        );
    }

    #[test]
    fn tags_start_at_one_and_each_is_used_once() {
        let (_, message) = refused(|s| {
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(0),
                line: Geometry(0),
            };
        });
        assert_eq!(
            message,
            "constraint 1 (tag 0): a horizontal constraint's tag must be from 1 to 2147483647 (the solver never reports tag 0, and takes tags as C ints), got 0"
        );
        let (_, message) = refused(|s| {
            s.constraints[3] = SolverConstraint::Radius {
                tag: Tag(1),
                circle: Geometry(1),
                value: Value(17),
            };
        });
        assert_eq!(message, "tag 1 is used twice: each constraint has its own");
    }

    #[test]
    fn an_arcs_rules_need_an_unknown_in_each_equation() {
        // FreeCAD c6de527391: rules on a fixed arc are rows of zeros that count against the
        // degrees of freedom (a fixed arc and a point on its start: dofs -4, not 0).
        let (_, message) = refused(|s| s.unknowns.retain(|v| !(7..16).contains(&v.0)));
        assert_eq!(
            message,
            "constraint 0: an arc's rules on arc 2, whose start's x equation has no unknown: rules go only on an arc the solver may change"
        );
        // Each equation on its own: the arc's values are the centre (7, 8), the radius (9),
        // the angles (10, 11), the start (12, 13), and the end (14, 15).
        for (fixed, row) in [
            ([7, 9, 10, 12], "start's x"),
            ([8, 9, 10, 13], "start's y"),
            ([7, 9, 11, 14], "end's x"),
            ([8, 9, 11, 15], "end's y"),
        ] {
            let (_, message) = refused(|s| s.unknowns.retain(|v| !fixed.contains(&v.0)));
            assert!(
                message.contains(&format!("whose {row} equation has no unknown")),
                "{row}: {message}"
            );
        }
        // An unknown in each is enough: only the start and end points free (12 to 15), the
        // centre, radius, and angles fixed.
        let mut s = system();
        s.unknowns.retain(|v| !(7..12).contains(&v.0));
        s.check().unwrap();
    }

    #[test]
    fn algorithms_are_one_to_three_different_ones() {
        use Algorithm::{Bfgs, DogLeg, LevenbergMarquardt};
        for algorithms in [
            vec![DogLeg],
            vec![LevenbergMarquardt, DogLeg],
            vec![DogLeg, LevenbergMarquardt, Bfgs],
        ] {
            let mut s = system();
            s.algorithms = algorithms;
            s.check().unwrap();
        }
        for algorithms in [
            vec![],
            vec![DogLeg, DogLeg],
            vec![Bfgs, DogLeg, Bfgs, LevenbergMarquardt],
        ] {
            let (parameter, message) = refused(|s| s.algorithms = algorithms.clone());
            assert_eq!(parameter, "algorithms");
            assert_eq!(
                message,
                format!("algorithms must be one to three different ones, got {algorithms:?}")
            );
        }
    }

    #[test]
    fn an_arcs_rules_are_given_once() {
        let (_, message) = refused(|s| {
            s.constraints
                .push(SolverConstraint::ArcRules { arc: Geometry(2) })
        });
        assert_eq!(message, "arc 2's rules are given twice");
    }

    #[test]
    fn sizes_past_the_limits_are_refused() {
        let (parameter, message) = refused(|s| s.values.resize(MAX_SKETCH_VALUES + 1, 0.0));
        assert_eq!(parameter, "values");
        assert_eq!(
            message,
            format!("a sketch may have at most {MAX_SKETCH_VALUES} values, got 10001")
        );
        let mut s = system();
        s.values.resize(MAX_SKETCH_VALUES, 0.0);
        s.check().unwrap();

        let horizontal = |t: u32| SolverConstraint::Horizontal {
            tag: Tag(t),
            line: Geometry(0),
        };
        let (parameter, message) = refused(|s| {
            s.constraints = (1..=MAX_SKETCH_CONSTRAINTS as u32 + 1)
                .map(horizontal)
                .collect();
        });
        assert_eq!(parameter, "constraints");
        assert_eq!(
            message,
            format!("a sketch may have at most {MAX_SKETCH_CONSTRAINTS} constraints, got 10001")
        );
    }

    #[test]
    fn a_jacobian_past_the_limit_is_refused_and_one_at_it_passes() {
        // The limit is on rows × unknowns, PlaneGCS's dense Jacobian. A coincidence is two
        // rows: 8,192 of them on 2,048 unknowns is 2^25 exactly; one more is past it.
        let with = |coincidences: u32| {
            let mut s = system();
            s.values.resize(MAX_SKETCH_UNKNOWNS, 1.0);
            s.unknowns = (0..MAX_SKETCH_UNKNOWNS as u32).map(Value).collect();
            s.constraints = vec![SolverConstraint::ArcRules { arc: Geometry(2) }];
            s.constraints
                .extend((1..=coincidences).map(|t| SolverConstraint::Coincident {
                    tag: Tag(t),
                    a: PointOf::Start(Geometry(0)),
                    b: PointOf::End(Geometry(0)),
                }));
            s
        };
        // The arc's rules are 4 rows too.
        with(8190).check().unwrap();
        let e = with(8191).check().unwrap_err();
        assert_eq!(e.parameter, "constraints");
        assert_eq!(
            e.message,
            format!(
                "16386 equations in 2048 unknowns is more than the solver can diagnose (at most {MAX_JACOBIAN_ENTRIES} entries)"
            )
        );
    }

    #[test]
    fn unknowns_and_geometry_past_their_limits_are_refused() {
        // Every value an unknown but the two dimensions' (16 and 17).
        let mut s = system();
        s.values.resize(MAX_SKETCH_UNKNOWNS + 3, 1.0);
        s.unknowns = (0..MAX_SKETCH_UNKNOWNS as u32 + 2)
            .filter(|v| !matches!(v, 16 | 17))
            .map(Value)
            .collect();
        s.check().unwrap();
        s.unknowns.push(Value(MAX_SKETCH_UNKNOWNS as u32 + 2));
        let e = s.check().unwrap_err();
        assert_eq!(e.parameter, "unknowns");
        assert_eq!(
            e.message,
            format!("a sketch may have at most {MAX_SKETCH_UNKNOWNS} unknowns, got 2049")
        );

        let mut s = system();
        let point = SketchGeometry::Point(PointValues {
            x: Value(0),
            y: Value(1),
        });
        s.geometry.resize(MAX_SKETCH_GEOMETRY, point);
        s.check().unwrap();
        s.geometry.push(point);
        let e = s.check().unwrap_err();
        assert_eq!(e.parameter, "geometry");
        assert_eq!(
            e.message,
            format!(
                "a sketch may have at most {MAX_SKETCH_GEOMETRY} pieces of geometry, got 10001"
            )
        );
    }

    #[test]
    fn a_tag_past_a_c_int_is_refused_and_the_largest_passes() {
        // PlaneGCS takes tags as C ints, and a negative one drops out of its diagnosis.
        let tagged = |t: u32| {
            let mut s = system();
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(t),
                line: Geometry(0),
            };
            s
        };
        tagged(MAX_TAG).check().unwrap();
        for t in [MAX_TAG + 1, u32::MAX] {
            let e = tagged(t).check().unwrap_err();
            assert_eq!(e.parameter, "constraints");
            assert_eq!(
                e.message,
                format!(
                    "constraint 1 (tag {t}): a horizontal constraint's tag must be from 1 to 2147483647 (the solver never reports tag 0, and takes tags as C ints), got {t}"
                )
            );
        }
    }

    #[test]
    fn a_line_drawn_with_no_length_is_refused_and_one_of_any_length_passes() {
        let mut s = system();
        s.values[2] = 0.0;
        let e = s.check().unwrap_err();
        assert_eq!(e.parameter, "geometry");
        assert_eq!(
            e.message,
            "geometry 0 is a line drawn with no length, its ends at (0, 0) and (0, 0)"
        );
        // 1e-200 squared underflows to 0: no length to the solver.
        s.values[2] = 1e-200;
        let e = s.check().unwrap_err();
        assert!(
            e.message
                .starts_with("geometry 0 is a line drawn with no length")
        );
        // 1e-150 squared is 1e-300: a length.
        s.values[2] = 1e-150;
        s.check().unwrap();
    }

    #[test]
    fn a_system_crosses_the_wire_as_it_was() {
        let s = system();
        let bytes = postcard::to_stdvec(&s).unwrap();
        let back: SketchSystem = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, s);
    }
}
