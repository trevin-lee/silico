//! A solved sketch's profile (docs/design/m3.5-profile.md; docs/ports/profile.md, *What the
//! core computes*): its loops, each an outline its curves walk in order, nested into regions
//! (an outer loop counter-clockwise and its holes clockwise), or what keeps it from being
//! one, named. It's pure computation on the solve's coordinates, never the stored ones, so
//! it needs no worker; the kernel only builds the faces it's told to (decision 2).
//!
//! What's refused, in the order it's checked, each an `invalid_input` naming the sketch and
//! the elements at fault:
//! 1. no profile curve (none, or construction only);
//! 2. a curve too small to build: a radius, or a line's or an arc's ends apart, of at most
//!    the tolerance (`silico_protocol::Curve::size_problem`, the kernel request's own rule);
//! 3. an open end (a point that ends one profile curve), or open ends close enough to be one
//!    point ("share the point", D2);
//! 4. a point where three or more profile curves meet;
//! 5. two curves that meet away from a point they share: crossing, overlapping, or within
//!    the tolerance of each other;
//! 6. a loop, or a region (its outer loop less its holes), with no area to speak of.
//!
//! 3 to 5 are listed together; 6 only once they pass, since it needs the loops. Every
//! refusal of a kind is listed, up to [`PROBLEMS_LISTED`] in all, and the rest counted.
//!
//! The tolerance is OCCT's, [`LENGTH_TOLERANCE`] (1e-7 mm), inclusive: every vertex, edge,
//! and face the kernel builds carries it, so two curves within it are one place in the
//! B-rep (the maintainer's decision, 2026-10-18, on the design's open question 1). Every
//! closeness question measures by the request's `distance` (`libm`'s `hypot`) and asks
//! whether it's within the tolerance (two curves that close meet, and two open ends that
//! close should share a point), or whether a loop or region is a sliver (the request's
//! `sliver`). Nothing is joined or moved. Every rule the kernel's faces request checks
//! (`silico_protocol::FaceRegions::check`) the profile applies first, by the request's own
//! code on the request's own numbers: each curve is converted to the request's form by
//! one conversion, and kept on its edge ([`Edge::sent`], which [`Profile::faces`] sends as
//! it is); its size
//! by `Curve::size_problem`, an arc's ends by `Curve::off_circle`, and each loop's area and
//! perimeter by `area_and_perimeter`, so a profile made is a request taken. Nothing here
//! depends on the order the sketch lists its geometry: loops, regions, and refusals are
//! ordered by ID.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use silico_protocol::{
    LENGTH_TOLERANCE, SizeProblem, arc_from_points, area_and_perimeter, distance, format_number,
    loop_area, side_of_line, sliver,
};

use crate::document::{Checked, ElementId, FeatureId, Operation, PROBLEMS_LISTED, Plane, Sketch};
use crate::sketch::SketchSolution;
use crate::{Error, ErrorCode};

/// Whether two things `distance` apart (by `silico_protocol::distance`, the request's own
/// measure) are one place to OCCT: within [`LENGTH_TOLERANCE`], inclusive. Two curves that
/// close meet, and two open ends that close should share a point (D2).
fn touching(distance: f64) -> bool {
    distance <= LENGTH_TOLERANCE
}

/// What kind of curve an edge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurveKind {
    Line,
    Arc,
    Circle,
}

impl CurveKind {
    /// As messages name it.
    pub fn noun(self) -> &'static str {
        match self {
            CurveKind::Line => "line",
            CurveKind::Arc => "arc",
            CurveKind::Circle => "circle",
        }
    }
}

/// One curve of a loop, as the loop walks it.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// The sketch's curve.
    pub curve: ElementId,
    pub kind: CurveKind,
    /// An arc's or a circle's centre; none for a line.
    pub center: Option<ElementId>,
    /// The points the walk goes from and to; none for a circle, which has no ends.
    pub ends: Option<[ElementId; 2]>,
    /// Whether the walk runs against the curve as drawn: a line from its `to` to its
    /// `from`, an arc clockwise from its end to its start, a circle clockwise.
    pub reversed: bool,
    /// The curve as the kernel's faces request takes it, walked this way: by the one
    /// conversion, and what its loop's area and perimeter were measured from
    /// (`silico_protocol::area_and_perimeter`, the request's own measure).
    pub sent: silico_protocol::Curve,
}

/// A closed outline: its curves in walking order, from the one with the smallest ID, each
/// starting where the one before ends (and the first where the last ends).
#[derive(Debug, Clone, PartialEq)]
pub struct Loop {
    pub curves: Vec<Edge>,
    /// The area it encloses, in mm², signed by the way it's walked: above 0 for
    /// counter-clockwise (an outer loop), below for clockwise (a hole).
    pub area: f64,
    /// Its length, in mm.
    pub perimeter: f64,
}

impl Loop {
    /// Its curves as the kernel's faces request takes them, in walking order
    /// ([`Edge::sent`]).
    pub fn sent(&self) -> Vec<silico_protocol::Curve> {
        self.curves.iter().map(|e| e.sent).collect()
    }
}

/// A face to be: an outer loop, counter-clockwise, and the holes in it, clockwise.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub outer: Loop,
    /// By their smallest curve ID.
    pub holes: Vec<Loop>,
    /// How many loops are around the outer one: 0 for an outline in the open, 2 for an
    /// island in a hole, and so on.
    pub depth: usize,
}

impl Region {
    /// The area the region covers, in mm²: its outer loop's less its holes'.
    pub fn area(&self) -> f64 {
        self.outer.area + self.holes.iter().map(|h| h.area).sum::<f64>()
    }

    /// The length of its outline, in mm: its outer loop's and its holes'.
    pub fn perimeter(&self) -> f64 {
        self.outer.perimeter + self.holes.iter().map(|h| h.perimeter).sum::<f64>()
    }
}

/// A sketch's profile: its regions, by their outer loops' smallest curve IDs. Several are
/// several faces; whether a feature takes them is the feature's (M3.6).
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// The sketch's plane, which its faces are made on.
    pub plane: Plane,
    pub regions: Vec<Region>,
}

/// Why a sketch makes no profile: its refusals, in the order they're checked, the first
/// [`PROBLEMS_LISTED`] of them listed, and how many there are; and every element any of
/// them names, listed or not, by ID.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    errors: Vec<Error>,
    total: u64,
    elements: BTreeSet<String>,
}

impl Refused {
    /// The refusals listed: every one, or the first [`PROBLEMS_LISTED`].
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// How many there are, listed or not.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The elements at fault, every refusal's, by ID: every one, or the first
    /// [`PROBLEMS_LISTED`].
    pub fn elements(&self) -> Vec<String> {
        self.elements
            .iter()
            .take(PROBLEMS_LISTED)
            .cloned()
            .collect()
    }

    /// How many elements are at fault, listed or not.
    pub fn elements_total(&self) -> u64 {
        self.elements.len() as u64
    }

    fn of(error: Error) -> Self {
        Refused {
            elements: error.elements().iter().cloned().collect(),
            errors: vec![error],
            total: 1,
        }
    }
}

/// Refusals as they're found: the first [`PROBLEMS_LISTED`] kept, the rest counted, and
/// every one's elements, kept or not.
#[derive(Default)]
struct Found {
    errors: Vec<Error>,
    total: u64,
    elements: BTreeSet<String>,
}

impl Found {
    /// A refusal of `elements`: made by `error`, given them, if it's among the first
    /// listed.
    fn add(&mut self, elements: Vec<String>, error: impl FnOnce(Vec<String>) -> Error) {
        self.total += 1;
        self.elements.extend(elements.iter().cloned());
        if self.errors.len() < PROBLEMS_LISTED {
            self.errors.push(error(elements));
        }
    }

    /// Counts refusals that weren't made into errors, of these elements.
    fn count(&mut self, unlisted: u64, elements: impl IntoIterator<Item = String>) {
        self.total += unlisted;
        self.elements.extend(elements);
    }

    fn into_result(self) -> Result<(), Refused> {
        if self.total == 0 {
            Ok(())
        } else {
            Err(Refused {
                errors: self.errors,
                total: self.total,
                elements: self.elements,
            })
        }
    }
}

type Xy = [f64; 2];

fn sub(a: Xy, b: Xy) -> Xy {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Xy, b: Xy) -> Xy {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: Xy, k: f64) -> Xy {
    [a[0] * k, a[1] * k]
}

fn dot(a: Xy, b: Xy) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Xy, b: Xy) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// An arc: counter-clockwise from `start` to `end`, from angle `from` to `to` (radians,
/// `from` in (−π, π], `to` past it by the sweep, less than a turn).
#[derive(Debug, Clone, Copy)]
struct Arc {
    start: Xy,
    end: Xy,
    from: f64,
    to: f64,
}

/// A curve's geometry, solved.
#[derive(Debug, Clone, Copy)]
enum Shape {
    Line {
        a: Xy,
        b: Xy,
    },
    /// A circle about `c` of radius `r`, or an arc of it.
    Round {
        c: Xy,
        r: f64,
        arc: Option<Arc>,
    },
}

impl Shape {
    /// Its bounding box, `[min x, min y, max x, max y]`: a round's is its circle's.
    fn bounds(&self) -> [f64; 4] {
        match *self {
            Shape::Line { a, b } => [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ],
            Shape::Round { c, r, .. } => [c[0] - r, c[1] - r, c[0] + r, c[1] + r],
        }
    }
}

/// A profile curve: what the sketch calls it, and where the solve put it.
struct ProfileCurve<'a> {
    id: &'a ElementId,
    kind: CurveKind,
    center: Option<&'a ElementId>,
    /// Its ends as drawn (a line's from and to, an arc's start and end), by place in the
    /// points table; none for a circle.
    ends: Option<[usize; 2]>,
    shape: Shape,
}

impl ProfileCurve<'_> {
    fn named(&self) -> String {
        format!("{} {}", self.kind.noun(), self.id)
    }

    /// The curve as drawn, for [`sent`].
    fn drawn(&self) -> Drawn {
        match self.shape {
            Shape::Line { a, b } => Drawn::Line { from: a, to: b },
            Shape::Round {
                c, arc: Some(arc), ..
            } => Drawn::Arc {
                center: c,
                start: arc.start,
                end: arc.end,
            },
            Shape::Round { c, r, arc: None } => Drawn::Circle {
                center: c,
                radius: r,
            },
        }
    }
}

/// A profile curve as the sketch draws it, where the solve put it. The gate measures a
/// sketch's curves by the same conversion ([`sent`]), drawn where the file stores them.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Drawn {
    Line {
        from: Xy,
        to: Xy,
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: Xy,
        start: Xy,
        end: Xy,
    },
    Circle {
        center: Xy,
        radius: f64,
    },
}

/// `p` with a zero of either sign as +0, so a point is one point, and an arc one angle,
/// whatever the sign of its zeros (a solve can leave −0, and atan2(±0, −1) is ±π).
pub(crate) fn unsigned([x, y]: Xy) -> Xy {
    [x + 0.0, y + 0.0]
}

/// The one conversion of a profile curve to the kernel request's: walked as drawn, or
/// `reversed`. A line goes from its walk's first point to its second; an arc has its
/// radius and angles from its own points, as M3.3's translation has them
/// (`arc_from_points`, the libm crate's), and one walked against its drawing is sent end
/// first with its angles swapped, so it turns clockwise; a circle has its radius, and no
/// direction (the request runs it with its loop's). Its points' zeros are +0 already, as
/// [`gather`] puts them. None for an arc whose points make none (its ends at one angle from
/// its centre).
pub(crate) fn sent(drawn: Drawn, reversed: bool) -> Option<silico_protocol::Curve> {
    use silico_protocol::Curve;
    Some(match drawn {
        Drawn::Line { from, to } => {
            let (start, end) = if reversed { (to, from) } else { (from, to) };
            Curve::Line { start, end }
        }
        Drawn::Arc { center, start, end } => {
            let [radius, start_angle, end_angle] = arc_from_points(center, start, end)?;
            if reversed {
                Curve::Arc {
                    center,
                    start: end,
                    end: start,
                    radius,
                    start_angle: end_angle,
                    end_angle: start_angle,
                }
            } else {
                Curve::Arc {
                    center,
                    start,
                    end,
                    radius,
                    start_angle,
                    end_angle,
                }
            }
        }
        Drawn::Circle { center, radius } => Curve::Circle { center, radius },
    })
}

/// A walk's curves as the request takes them: none if one can't be converted.
fn sent_loop(
    curves: &[ProfileCurve<'_>],
    edges: &[(usize, bool)],
) -> Option<Vec<silico_protocol::Curve>> {
    edges
        .iter()
        .map(|&(c, reversed)| sent(curves[c].drawn(), reversed))
        .collect()
}

/// The profile of `feature`, a sketch of `document`, from `solution`, its solve: the
/// regions its non-construction curves make, or why they make none, as `invalid_input`
/// errors naming the feature and the elements at fault. The sketch is one the gate took,
/// so its size is bounded: at most `MAX_SKETCH_UNKNOWNS` (1,024) unknowns, and a loop
/// costs at least 3 (a circle's centre and radius), so at most 341 loops. That bound
/// matters to the kernel, whose face check grows with the square of a face's holes
/// (OCCT, optimized: 1,000 holes 3.7 s, 3,000 holes 24 s).
pub fn profile(
    document: &Checked,
    feature: &FeatureId,
    solution: &SketchSolution,
) -> Result<Profile, Refused> {
    let found = document
        .document()
        .features
        .iter()
        .find(|f| &f.id == feature)
        .ok_or_else(|| {
            Refused::of(Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {}", feature.as_str()),
            ))
        })?;
    let Operation::Sketch(sketch) = &found.operation else {
        return Err(Refused::of(
            Error::new(
                ErrorCode::InvalidInput,
                format!("feature {} isn't a sketch", feature.as_str()),
            )
            .with_feature(feature.as_str()),
        ));
    };
    of_sketch(feature, sketch, solution)
}

/// `sketch`'s profile ([`profile`]).
fn of_sketch(
    feature: &FeatureId,
    sketch: &Sketch,
    solution: &SketchSolution,
) -> Result<Profile, Refused> {
    let _span = tracing::debug_span!(
        "profile",
        feature = feature.as_str(),
        lines = sketch.lines.len(),
        circles = sketch.circles.len(),
        arcs = sketch.arcs.len()
    )
    .entered();
    let refuse = |message: String, elements: Vec<String>, suggestion: &str| {
        Error::new(ErrorCode::InvalidInput, message)
            .with_feature(feature.as_str())
            .with_elements(elements)
            .with_suggestion(suggestion)
    };

    let Gathered {
        points,
        ids,
        curves,
        unbuilt,
    } = gather(feature, sketch, solution)?;
    if curves.is_empty() && unbuilt.is_empty() {
        return Err(Refused::of(refuse(
            "the sketch has no curves to make a profile of".to_owned(),
            Vec::new(),
            "draw a closed outline of lines, arcs, or circles that aren't construction geometry",
        )));
    }

    // 2. Curves the request can't build, by its own rules on the curve as it would be
    // sent: too small, or an arc with an end off its circle; and arcs whose points make
    // none. By curve ID.
    let mut small: BTreeMap<&ElementId, Error> = unbuilt.into_iter().collect();
    for curve in &curves {
        let Some(as_sent) = sent(curve.drawn(), false) else {
            continue;
        };
        let (problem, off) = (as_sent.size_problem(), as_sent.off_circle());
        let mut elements = vec![curve.id.to_string()];
        for p in curve
            .center
            .iter()
            .map(|id| id.to_string())
            .chain(curve.ends.into_iter().flatten().map(|p| ids[p].to_string()))
        {
            if !elements.contains(&p) {
                elements.push(p);
            }
        }
        let rule = format!(
            "a profile's lines and arcs are longer than {} mm between their ends",
            format_number(LENGTH_TOLERANCE)
        );
        let error = match (problem, off, curve.kind) {
            (None, None, _) => continue,
            (None, Some((end, d)), _) => refuse(
                format!(
                    "{}'s {} is {} mm from its circle (the circle about its centre through its start): a profile's arcs end within {} mm of their circles",
                    curve.named(),
                    end.as_str(),
                    format_number(d),
                    format_number(LENGTH_TOLERANCE)
                ),
                elements,
                "solve the sketch again, or put the arc's end on its circle",
            ),
            (Some(SizeProblem::Ends(d)), _, CurveKind::Line) => refuse(
                format!("{} is {} mm long: {rule}", curve.named(), format_number(d)),
                elements,
                "lengthen it, or remove it and end its neighbours at one point",
            ),
            (Some(SizeProblem::Ends(d)), _, _) => refuse(
                format!(
                    "{}'s ends are {} mm apart: {rule}",
                    curve.named(),
                    format_number(d)
                ),
                elements,
                "draw a circle instead, or open the arc",
            ),
            (Some(SizeProblem::Radius(r)), _, _) => refuse(
                format!(
                    "{}'s radius is {} mm: a profile's arcs and circles have a radius above {} mm",
                    curve.named(),
                    format_number(r),
                    format_number(LENGTH_TOLERANCE)
                ),
                elements,
                "draw it larger, or remove it",
            ),
        };
        small.insert(curve.id, error);
    }
    let mut found = Found::default();
    for (_, error) in small {
        found.add(error.elements().to_vec(), |_| error);
    }
    found.into_result()?;

    let mut found = Found::default();
    // Each point's curves, once for each end of theirs there.
    let mut at_point: Vec<Vec<usize>> = vec![Vec::new(); points.len()];
    for (i, curve) in curves.iter().enumerate() {
        for end in curve.ends.into_iter().flatten() {
            at_point[end].push(i);
        }
    }
    let point_name = |p: usize| ids[p].to_string();

    // 3. Open ends, and open ends that should be one point (never one curve's own two:
    // those are a curve too small, refused above).
    let open: Vec<usize> = (0..points.len())
        .filter(|&p| at_point[p].len() == 1)
        .collect();
    let groups = one_point_groups(&open, &points);
    // Each point as the meet test takes it: an open end grouped with others is the group's
    // first, so curves ending there are neighbours, not a meeting.
    let mut shared_as: Vec<usize> = (0..points.len()).collect();
    for group in &groups {
        for &p in group {
            shared_as[p] = group[0];
        }
    }
    for group in &groups {
        // Each curve once, though both its ends were in the group.
        let mut curves_there: Vec<String> = Vec::new();
        for id in group
            .iter()
            .flat_map(|&p| at_point[p].iter().map(|&c| curves[c].id.to_string()))
        {
            if !curves_there.contains(&id) {
                curves_there.push(id);
            }
        }
        if let [p] = group.as_slice() {
            let curve = &curves[at_point[*p][0]];
            let elements = vec![point_name(*p), curve.id.to_string()];
            found.add(elements, |elements| {
                refuse(
                    format!(
                        "point {} ends {} and no other profile curve, so the profile is open there",
                        ids[*p],
                        curve.named()
                    ),
                    elements,
                    "end another curve at the point to close the outline, or make the curve construction geometry",
                )
            });
        } else {
            let names: Vec<String> = group.iter().map(|&p| point_name(p)).collect();
            // A curve with both its ends here would shrink to nothing if they shared a
            // point: it's to be lengthened, not closed up.
            let mut both_ends: Vec<String> = Vec::new();
            for &c in group.iter().flat_map(|&p| &at_point[p]) {
                let curve = &curves[c];
                let named = curve.named();
                if curve
                    .ends
                    .is_some_and(|ends| ends.iter().all(|p| group.contains(p)))
                    && !both_ends.contains(&named)
                {
                    both_ends.push(named);
                }
            }
            let suggestion = if both_ends.is_empty() {
                "share the point: end the curves at one point".to_owned()
            } else {
                format!(
                    "lengthen {}, or end the curves at one point",
                    both_ends.join(" and ")
                )
            };
            let elements: Vec<String> = names.iter().cloned().chain(curves_there).collect();
            found.add(elements, |elements| {
                refuse(
                    format!(
                        "points {} end curves within {} mm of each other, but aren't one point, so the profile is open there",
                        names.join(", "),
                        format_number(LENGTH_TOLERANCE)
                    ),
                    elements,
                    &suggestion,
                )
            });
        }
    }

    // 4. Points where three or more curves meet.
    for (p, there) in at_point.iter().enumerate() {
        if there.len() >= 3 {
            let mut named: Vec<&ElementId> = there.iter().map(|&c| curves[c].id).collect();
            named.dedup();
            let elements: Vec<String> = std::iter::once(point_name(p))
                .chain(named.iter().map(|id| id.to_string()))
                .collect();
            found.add(elements, |elements| {
                refuse(
                    format!(
                        "{} curve ends meet at point {}: a profile's points each join two curves",
                        there.len(),
                        ids[p]
                    ),
                    elements,
                    "draw each outline so its points join two curves each, or make the extra curves construction geometry",
                )
            });
        }
    }

    // 5. Curves that meet away from a point they share.
    let (meets, unlisted, unkept) = meetings(&curves, &points, &shared_as);
    let rule = format!(
        "a profile's curves must be more than {} mm apart except at a point they share",
        format_number(LENGTH_TOLERANCE)
    );
    let redraw = "move or remove one of them: a profile's curves meet only at the points they share, and each point joins two curves";
    let part_name = |part: &Part| match *part {
        Part::Curve(c) => curves[c].id.to_string(),
        Part::Point(p) => point_name(p),
    };
    for (&(a, b), &how) in &meets {
        let elements: Vec<String> = how.parts(a, b).iter().map(part_name).collect();
        let (a, b) = (&curves[a], &curves[b]);
        found.add(elements, |elements| match how {
            Meeting::Cross => refuse(
                format!("{} and {} cross", a.named(), b.named()),
                elements,
                redraw,
            ),
            Meeting::Overlap => refuse(
                format!("{} and {} overlap", a.named(), b.named()),
                elements,
                redraw,
            ),
            Meeting::Apart(d) => refuse(
                format!(
                    "{} and {} are {} mm apart: {rule}",
                    a.named(),
                    b.named(),
                    format_number(d)
                ),
                elements,
                &format!(
                    "move them more than {} mm apart",
                    format_number(LENGTH_TOLERANCE)
                ),
            ),
            Meeting::End {
                point,
                of,
                distance,
            } => {
                let (end, other) = if of == 0 { (a, b) } else { (b, a) };
                refuse(
                    format!(
                        "point {}, the end of {}, is {} mm from {}: {rule}",
                        ids[point],
                        end.named(),
                        format_number(distance),
                        other.named()
                    ),
                    elements,
                    &format!(
                        "move point {} more than {} mm from {}, or redraw so they share a point",
                        ids[point],
                        format_number(LENGTH_TOLERANCE),
                        other.id
                    ),
                )
            }
        });
    }
    found.count(unlisted, unkept.iter().map(part_name));
    found.into_result()?;

    // The loops, then 6: loops and regions with no area, measured as the request measures
    // them.
    let loops =
        walk(&curves, &at_point).map_err(|e| Refused::of(e.with_feature(feature.as_str())))?;
    let profile = regions(&curves, &ids, &loops, sketch.plane)
        .map_err(|e| Refused::of(e.with_feature(feature.as_str())))?;
    let named =
        |l: &Loop| -> Vec<String> { l.curves.iter().map(|e| e.curve.to_string()).collect() };
    let thin = format!(
        "give it some width: the kernel builds nothing thinner than {} mm",
        format_number(LENGTH_TOLERANCE)
    );
    let mut found = Found::default();
    let all = profile
        .regions
        .iter()
        .flat_map(|r| std::iter::once(&r.outer).chain(&r.holes));
    let mut any_thin = false;
    for l in all {
        if sliver(l.area, l.perimeter) {
            any_thin = true;
            found.add(named(l), |elements| {
                refuse(
                    format!(
                        "loop {} encloses no area to speak of ({} mm², against a perimeter of {} mm)",
                        elements.join(", "),
                        format_number(l.area.abs()),
                        format_number(l.perimeter)
                    ),
                    elements,
                    &thin,
                )
            });
        }
    }
    // A region of loops that each have area can still have none: a ring.
    if !any_thin {
        for region in &profile.regions {
            if !region.holes.is_empty() && sliver(region.area(), region.perimeter()) {
                let elements: Vec<String> = std::iter::once(&region.outer)
                    .chain(&region.holes)
                    .flat_map(named)
                    .collect();
                found.add(elements, |elements| {
                    refuse(
                        format!(
                            "the region inside loop {} less its holes encloses no area to speak of ({} mm², against a perimeter of {} mm)",
                            named(&region.outer).join(", "),
                            format_number(region.area().abs()),
                            format_number(region.perimeter())
                        ),
                        elements,
                        &thin,
                    )
                });
            }
        }
    }
    found.into_result()?;
    Ok(profile)
}

/// A sketch's profile curves and their points, where the solve put them.
struct Gathered<'a> {
    /// The points table: the curves' ends and centres, in ID order.
    points: Vec<Xy>,
    /// Each point's ID, by place in the table.
    ids: Vec<&'a ElementId>,
    /// The profile curves, in ID order.
    curves: Vec<ProfileCurve<'a>>,
    /// Arcs whose solved points make none (their ends at one angle from their centre),
    /// left out of `curves`, each with its refusal.
    unbuilt: Vec<(&'a ElementId, Error)>,
}

/// The points and profile curves of `sketch` where `solution` puts them, each point's
/// zeros as +0. Refused where the solution doesn't hold the sketch (a bug).
fn gather<'a>(
    feature: &FeatureId,
    sketch: &'a Sketch,
    solution: &SketchSolution,
) -> Result<Gathered<'a>, Refused> {
    let lines: Vec<_> = sketch.lines.iter().filter(|l| !l.construction).collect();
    let circles: Vec<_> = sketch.circles.iter().filter(|c| !c.construction).collect();
    let arcs: Vec<_> = sketch.arcs.iter().filter(|a| !a.construction).collect();

    let mut table: BTreeMap<&ElementId, Xy> = BTreeMap::new();
    let mut missing = None;
    let used = lines
        .iter()
        .flat_map(|l| [(&l.from, &l.id), (&l.to, &l.id)])
        .chain(circles.iter().map(|c| (&c.center, &c.id)))
        .chain(
            arcs.iter()
                .flat_map(|a| [(&a.center, &a.id), (&a.start, &a.id), (&a.end, &a.id)]),
        );
    for (point, curve) in used {
        match solution.point(point) {
            Some(xy) => {
                table.insert(point, unsigned(xy));
            }
            None => {
                missing.get_or_insert((point, curve));
            }
        }
    }
    let unsolved = |what: String, elements: [&ElementId; 2]| {
        Refused::of(
            Error::new(
                ErrorCode::Internal,
                format!("the sketch's solution has no {what}"),
            )
            .with_feature(feature.as_str())
            .with_elements(elements.iter().map(|id| id.to_string()))
            .with_suggestion("solve the sketch again: its solution is of another sketch"),
        )
    };
    if let Some((point, curve)) = missing {
        return Err(unsolved(
            format!("point {point}, which curve {curve} is drawn from"),
            [point, curve],
        ));
    }
    let ids: Vec<&ElementId> = table.keys().copied().collect();
    let place: BTreeMap<&ElementId, usize> =
        ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let points: Vec<Xy> = table.values().copied().collect();
    let at = |id: &ElementId| place.get(id).copied().unwrap_or(0);

    let mut unbuilt = Vec::new();
    let mut curves = Vec::new();
    for l in lines {
        let (a, b) = (at(&l.from), at(&l.to));
        curves.push(ProfileCurve {
            id: &l.id,
            kind: CurveKind::Line,
            center: None,
            ends: Some([a, b]),
            shape: Shape::Line {
                a: points[a],
                b: points[b],
            },
        });
    }
    for c in circles {
        let Some(r) = solution.radius(&c.id) else {
            return Err(unsolved(
                format!("radius for circle {}", c.id),
                [&c.id, &c.center],
            ));
        };
        curves.push(ProfileCurve {
            id: &c.id,
            kind: CurveKind::Circle,
            center: Some(&c.center),
            ends: None,
            shape: Shape::Round {
                c: points[at(&c.center)],
                r,
                arc: None,
            },
        });
    }
    for a in arcs {
        let (c, s, e) = (at(&a.center), at(&a.start), at(&a.end));
        let Some([r, from, to]) = arc_from_points(points[c], points[s], points[e]) else {
            unbuilt.push((
                &a.id,
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "arc {}'s start and end are solved to one angle from its centre, so it sweeps nothing, or a whole turn",
                        a.id
                    ),
                )
                .with_feature(feature.as_str())
                .with_elements([&a.id, &a.center, &a.start, &a.end].map(|id| id.to_string()))
                .with_suggestion(
                    "move the arc's end round its centre from its start, or draw a circle",
                ),
            ));
            continue;
        };
        curves.push(ProfileCurve {
            id: &a.id,
            kind: CurveKind::Arc,
            center: Some(&a.center),
            ends: Some([s, e]),
            shape: Shape::Round {
                c: points[c],
                r,
                arc: Some(Arc {
                    start: points[s],
                    end: points[e],
                    from,
                    to,
                }),
            },
        });
    }
    curves.sort_by(|a, b| a.id.cmp(b.id));
    Ok(Gathered {
        points,
        ids,
        curves,
        unbuilt,
    })
}

/// The open ends, grouped: ends within the tolerance of each other (and so on, through
/// each other) together, each group in order and the groups by their first.
fn one_point_groups(open: &[usize], points: &[Xy]) -> Vec<Vec<usize>> {
    let mut by_x: Vec<usize> = (0..open.len()).collect();
    by_x.sort_by(|&a, &b| {
        points[open[a]][0]
            .total_cmp(&points[open[b]][0])
            .then(a.cmp(&b))
    });
    let mut parent: Vec<usize> = (0..open.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (k, &i) in by_x.iter().enumerate() {
        let p = points[open[i]];
        for &j in &by_x[k + 1..] {
            let q = points[open[j]];
            // Twice the tolerance: a pair this far apart along x is apart.
            if q[0] - p[0] > 2.0 * LENGTH_TOLERANCE {
                break;
            }
            if touching(distance(p, q)) {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri.max(rj)] = ri.min(rj);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &p) in open.iter().enumerate() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(p);
    }
    groups.into_values().collect()
}

/// How two curves meet away from the points they share.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Meeting {
    /// They cross: their lines or circles meet at a point on both, and pass through.
    Cross,
    /// They lie on one line or circle and run along each other.
    Overlap,
    /// They come within the tolerance of each other, at their nearest this far apart.
    Apart(f64),
    /// An end of one, `point`, not shared, is `distance` from the other; `of` is the curve
    /// it ends (0 the first of the pair, 1 the second).
    End {
        point: usize,
        of: usize,
        distance: f64,
    },
}

/// A part of the sketch a meeting names: a curve or a point, by its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Curve(usize),
    Point(usize),
}

impl Meeting {
    /// What a meeting of curves `a` and `b` names: the two curves, and for an end near the
    /// other curve, the curve it ends first, then the other, then the end's point.
    fn parts(self, a: usize, b: usize) -> Vec<Part> {
        match self {
            Meeting::End { point, of, .. } => {
                let (end, other) = if of == 0 { (a, b) } else { (b, a) };
                vec![Part::Curve(end), Part::Curve(other), Part::Point(point)]
            }
            Meeting::Cross | Meeting::Overlap | Meeting::Apart(_) => {
                vec![Part::Curve(a), Part::Curve(b)]
            }
        }
    }
}

/// Every pair of curves that meets away from a point they share, by their places (in ID
/// order), with how: the first [`PROBLEMS_LISTED`] pairs, how many more there are, and the
/// parts those more name ([`Meeting::parts`]), which are at fault too.
/// Pairs are found by a sweep over bounding boxes, whose order can't change which.
fn meetings(
    curves: &[ProfileCurve<'_>],
    points: &[Xy],
    shared_as: &[usize],
) -> (BTreeMap<(usize, usize), Meeting>, u64, BTreeSet<Part>) {
    let bounds: Vec<[f64; 4]> = curves.iter().map(|c| c.shape.bounds()).collect();
    let mut by_x: Vec<usize> = (0..curves.len()).collect();
    by_x.sort_by(|&a, &b| bounds[a][0].total_cmp(&bounds[b][0]).then(a.cmp(&b)));
    // Boxes this far apart hold curves that are apart: a box is off by at most an ulp or so
    // of its coordinates, under 2.4e-10 within reach (1e6 mm), far inside the margin's 1e-7
    // to spare. That holds only within reach: near 2^28 mm an ulp is 6e-8.
    let margin = 2.0 * LENGTH_TOLERANCE;
    let mut kept: BTreeMap<(usize, usize), Meeting> = BTreeMap::new();
    let mut total = 0u64;
    // The parts of the meetings not kept, which are at fault too: at most every curve and
    // point, however many meetings there are.
    let mut unkept: BTreeSet<Part> = BTreeSet::new();
    for (k, &i) in by_x.iter().enumerate() {
        for &j in &by_x[k + 1..] {
            if bounds[j][0] > bounds[i][2] + margin {
                break;
            }
            if bounds[j][1] > bounds[i][3] + margin || bounds[i][1] > bounds[j][3] + margin {
                continue;
            }
            let (a, b) = (i.min(j), i.max(j));
            if let Some(how) = meeting(&curves[a], &curves[b], points, shared_as) {
                total += 1;
                let listed = kept.len() < PROBLEMS_LISTED
                    || kept
                        .last_key_value()
                        .is_some_and(|(&last, _)| (a, b) < last);
                if listed {
                    kept.insert((a, b), how);
                    if kept.len() > PROBLEMS_LISTED
                        && let Some(((c, d), dropped)) = kept.pop_last()
                    {
                        unkept.extend(dropped.parts(c, d));
                    }
                } else {
                    unkept.extend(how.parts(a, b));
                }
            }
        }
    }
    let unlisted = total - kept.len() as u64;
    (kept, unlisted, unkept)
}

/// How curves `a` and `b` meet away from the points they share, if they do (the
/// decision's predicate 1), said as what's to be moved: along each other, then an end of
/// one near the other, then a crossing, then how near they come. Ends share a point when
/// `shared_as` takes them as one.
/// - Sharing none, they meet if they come within the tolerance anywhere.
/// - Sharing one or two, they meet if their lines or circles cross again on both curves,
///   further than the tolerance from the shared points; if an end of either, not shared,
///   is within the tolerance of the other; or if they lie on one line or circle and run
///   along each other beyond the shared points. Near a shared point they may be as close
///   as tangent neighbours are.
fn meeting(
    a: &ProfileCurve<'_>,
    b: &ProfileCurve<'_>,
    points: &[Xy],
    shared_as: &[usize],
) -> Option<Meeting> {
    let ends = |c: &ProfileCurve<'_>| -> ([usize; 2], usize) {
        match c.ends {
            Some(e) => (e, 2),
            None => ([0; 2], 0),
        }
    };
    let ((ends_a, na), (ends_b, nb)) = (ends(a), ends(b));
    let (ends_a, ends_b) = (&ends_a[..na], &ends_b[..nb]);
    // The points they share, as `shared_as` takes them: two at most.
    let mut shared_points = [0usize; 2];
    let mut count = 0;
    for &p in ends_a {
        let s = shared_as[p];
        if ends_b.iter().any(|&q| shared_as[q] == s) && !shared_points[..count].contains(&s) {
            shared_points[count] = s;
            count += 1;
        }
    }
    let shared = &shared_points[..count];

    if let Some(gap) = along(&a.shape, &b.shape) {
        return Some(if gap == 0.0 {
            Meeting::Overlap
        } else {
            Meeting::Apart(gap)
        });
    }
    // The end nearest the other curve, of those within the tolerance of it; the first by
    // ID where two are as near.
    let mut nearest: Option<Meeting> = None;
    for (of, ends, other) in [(0, ends_a, b), (1, ends_b, a)] {
        for &p in ends {
            if shared.contains(&shared_as[p]) {
                continue;
            }
            let d = point_distance(points[p], &other.shape);
            let nearer = match nearest {
                Some(Meeting::End {
                    point, distance, ..
                }) => d < distance || (d == distance && p < point),
                _ => true,
            };
            if touching(d) && nearer {
                nearest = Some(Meeting::End {
                    point: p,
                    of,
                    distance: d,
                });
            }
        }
    }
    if nearest.is_some() {
        return nearest;
    }
    let crossed = if shared.is_empty() {
        cross_apart(&a.shape, &b.shape)
    } else {
        // Each curve's own ends at the shared points.
        let at_shared = |c: &ProfileCurve<'_>| -> Vec<Shared> {
            let mut v = Vec::new();
            for (end, &p) in c.ends.iter().flatten().enumerate() {
                if shared.contains(&shared_as[p]) {
                    v.push(Shared { end, at: points[p] });
                }
            }
            v
        };
        cross_beside(&a.shape, &b.shape, &at_shared(a), &at_shared(b))
    };
    if crossed {
        return Some(Meeting::Cross);
    }
    if shared.is_empty() {
        let d = between(&a.shape, &b.shape);
        if touching(d) {
            return Some(Meeting::Apart(d));
        }
    }
    None
}

/// If the two curves lie on one line (each end of the shorter within the tolerance of the
/// longer's line) or one circle (centres and radii each within the tolerance) and share a
/// stretch longer than the tolerance, how far apart their lines or circles are at most:
/// 0 when they're exactly on one.
fn along(a: &Shape, b: &Shape) -> Option<f64> {
    match (*a, *b) {
        (Shape::Line { a: p, b: q }, Shape::Line { a: r, b: s }) => {
            let ((p, q), (r, s)) = if distance(r, s) > distance(p, q) {
                ((r, s), (p, q))
            } else {
                ((p, q), (r, s))
            };
            let u = sub(q, p);
            let length = distance(p, q);
            // Off the longer's line (exactly 0 for an end on it at either of its ends, or
            // on an axis), and along it from p.
            let off = |e: Xy| (cross(u, sub(e, p)) / length).abs();
            let at = |e: Xy| dot(u, sub(e, p)) / length;
            let gap = off(r).max(off(s));
            let (lo, hi) = (at(r).min(at(s)), at(r).max(at(s)));
            let shared = hi.min(length) - lo.max(0.0);
            (touching(gap) && !touching(shared)).then_some(gap)
        }
        (
            Shape::Round {
                c: c1,
                r: r1,
                arc: a1,
            },
            Shape::Round {
                c: c2,
                r: r2,
                arc: a2,
            },
        ) => {
            let (apart, radii) = (distance(c1, c2), (r1 - r2).abs());
            if !(touching(apart) && touching(radii)) {
                return None;
            }
            let sweep = |arc: Option<Arc>| arc.map_or((-PI, PI), |arc| (arc.from, arc.to));
            let ((f1, t1), (f2, t2)) = (sweep(a1), sweep(a2));
            let common: f64 = (-2..=2)
                .map(|k| {
                    let turn = f64::from(k) * TAU;
                    (t1.min(t2 + turn) - f1.max(f2 + turn)).max(0.0)
                })
                .sum();
            (!touching(r1.min(r2) * common)).then_some(apart + radii)
        }
        _ => None,
    }
}

/// The distance from `p` to a curve.
fn point_distance(p: Xy, shape: &Shape) -> f64 {
    match *shape {
        Shape::Line { a, b } => point_segment_distance(p, a, b),
        Shape::Round { c, r, arc } => {
            let d = distance(c, p);
            match arc {
                Some(arc) if !(d > 0.0 && on_arc(c, &arc, p)) => {
                    distance(p, arc.start).min(distance(p, arc.end))
                }
                _ => (d - r).abs(),
            }
        }
    }
}

/// The distance from `p` to the segment `a`–`b`: to whichever end is nearest where the
/// foot of `p` falls off the segment, measured from the end itself (so an end's distance
/// from an end is exactly theirs), and otherwise off the segment's line, by the cross
/// product.
fn point_segment_distance(p: Xy, a: Xy, b: Xy) -> f64 {
    let (u, w) = (sub(b, a), sub(p, a));
    let along = dot(w, u);
    if along <= 0.0 {
        distance(a, p)
    } else if along >= dot(u, u) {
        distance(b, p)
    } else {
        (cross(u, w) / distance(a, b)).abs()
    }
}

/// Whether `p`, a point of the arc's circle, is on the arc: counter-clockwise from its
/// start and clockwise from its end about `c`, by cross products. Which of the two rules
/// holds, between the ends or all but the rest of the turn, is the arc's sweep where its
/// ends are less than a quarter turn apart (it then turns almost nothing or almost a whole
/// circle, far from a half turn, and a cross product of its ends can round to 0, and
/// might round to the wrong sign), and the sign of their cross product near a half turn, where the sweep can
/// be π either side.
fn on_arc(c: Xy, arc: &Arc, p: Xy) -> bool {
    let (s, e, v) = (sub(arc.start, c), sub(arc.end, c), sub(p, c));
    let se = cross(s, e);
    let (sv, ve) = (cross(s, v), cross(v, e));
    // Less than a half turn: between the two.
    let minor = sv >= 0.0 && ve >= 0.0;
    // More: anywhere but strictly inside the rest of the turn, from end to start.
    let major = !(sv < 0.0 && ve < 0.0);
    if dot(s, e) > 0.0 {
        if arc.to - arc.from > PI { major } else { minor }
    } else if se > 0.0 {
        minor
    } else if se < 0.0 {
        major
    } else {
        // A half turn: the start's left.
        sv >= 0.0
    }
}

/// Whether `p`, a point of the round's circle, is on the round.
fn on_round(c: Xy, arc: Option<&Arc>, p: Xy) -> bool {
    arc.is_none_or(|arc| on_arc(c, arc, p))
}

/// The line through `p` and `q` against a circle about `c`: where the foot of the centre
/// is along it (p at 0, q at 1), the foot, and its distance from the centre.
fn foot(p: Xy, q: Xy, c: Xy) -> (f64, Xy, f64) {
    let u = sub(q, p);
    let t = dot(sub(c, p), u) / dot(u, u);
    let f = add(p, scale(u, t));
    (t, f, distance(f, c))
}

/// Whether two curves that share no point cross: two segments whose ends are each surely
/// on opposite sides of the other's line ([`side_of_line`]), or a line or a
/// circle passing through a circle at a point on both curves. A tangent, which touches
/// without passing through, is [`between`]'s.
fn cross_apart(a: &Shape, b: &Shape) -> bool {
    match (*a, *b) {
        (Shape::Line { a: p, b: q }, Shape::Line { a: r, b: s }) => {
            // Only sides rounding can't have chosen count: the ends of two sides on one
            // line, apart along it, are on it by rounding only, and their signs are
            // rounding's. Two that cross with an end too near the other's line to place
            // have met already: the crossing's stretch to that end passes the other's end
            // at most that end's distance from it, within the tolerance.
            side_of_line(p, q, r) * side_of_line(p, q, s) < 0
                && side_of_line(r, s, p) * side_of_line(r, s, q) < 0
        }
        (Shape::Line { a: p, b: q }, Shape::Round { c, r, arc })
        | (Shape::Round { c, r, arc }, Shape::Line { a: p, b: q }) => {
            let u = sub(q, p);
            let (t, _, d) = foot(p, q, c);
            if d >= r {
                return false;
            }
            let h = ((r - d) * (r + d)).sqrt() / dot(u, u).sqrt();
            [t - h, t + h]
                .into_iter()
                .any(|t| (0.0..=1.0).contains(&t) && on_round(c, arc.as_ref(), add(p, scale(u, t))))
        }
        (
            Shape::Round {
                c: c1,
                r: r1,
                arc: a1,
            },
            Shape::Round {
                c: c2,
                r: r2,
                arc: a2,
            },
        ) => {
            let w = sub(c2, c1);
            let d = distance(c1, c2);
            if d == 0.0 || d >= r1 + r2 || d <= (r1 - r2).abs() {
                return false;
            }
            let u = scale(w, 1.0 / d);
            // Along the line of centres from c1 to the chord through the crossings, and
            // half the chord.
            let along = (d + (r1 - r2) * (r1 + r2) / d) / 2.0;
            let half = ((r1 - along) * (r1 + along)).max(0.0).sqrt();
            let m = add(c1, scale(u, along));
            let across = [-u[1], u[0]];
            [-half, half].into_iter().any(|k| {
                let x = add(m, scale(across, k));
                on_round(c1, a1.as_ref(), x) && on_round(c2, a2.as_ref(), x)
            })
        }
    }
}

/// How near two curves that share no point and don't cross come between their ends (an
/// end's distance is the caller's): along the common normal, the line from a circle's
/// centre square to a segment, or the line of two circles' centres. Infinite for two
/// segments, which come nearest at an end.
fn between(a: &Shape, b: &Shape) -> f64 {
    match (*a, *b) {
        (Shape::Line { .. }, Shape::Line { .. }) => f64::INFINITY,
        (Shape::Line { a: p, b: q }, Shape::Round { c, r, arc })
        | (Shape::Round { c, r, arc }, Shape::Line { a: p, b: q }) => {
            let (t, f, d) = foot(p, q, c);
            if !(0.0..=1.0).contains(&t) || d == 0.0 {
                return f64::INFINITY;
            }
            let nearest = add(c, scale(sub(f, c), r / d));
            if on_round(c, arc.as_ref(), nearest) {
                (d - r).abs()
            } else {
                f64::INFINITY
            }
        }
        (
            Shape::Round {
                c: c1,
                r: r1,
                arc: a1,
            },
            Shape::Round {
                c: c2,
                r: r2,
                arc: a2,
            },
        ) => {
            let w = sub(c2, c1);
            let d = distance(c1, c2);
            if d == 0.0 {
                // On one centre: two circles are their radii apart; an arc's ends, the
                // caller's, are on the other if any of it is.
                return if a1.is_none() && a2.is_none() {
                    (r1 - r2).abs()
                } else {
                    f64::INFINITY
                };
            }
            let u = scale(w, 1.0 / d);
            let mut nearest = f64::INFINITY;
            for s1 in [1.0, -1.0] {
                for s2 in [1.0, -1.0] {
                    let (p1, p2) = (add(c1, scale(u, s1 * r1)), add(c2, scale(u, s2 * r2)));
                    if on_round(c1, a1.as_ref(), p1) && on_round(c2, a2.as_ref(), p2) {
                        nearest = nearest.min((s1 * r1 - d - s2 * r2).abs());
                    }
                }
            }
            nearest
        }
    }
}

/// One of a curve's ends at a point it shares with another curve.
#[derive(Clone, Copy)]
struct Shared {
    /// Which end: 0 for a line's from or an arc's start, 1 for the other.
    end: usize,
    /// Where this curve's end is.
    at: Xy,
}

/// Whether two curves that share points (each curve's own ends there) cross again, away
/// from them: where their lines and circles meet again, further than the tolerance from
/// every shared point, at a point on both. Two lines from one point meet again only along
/// each other, and two arcs of one circle only by running along each other ([`along`]'s).
fn cross_beside(a: &Shape, b: &Shape, shared_a: &[Shared], shared_b: &[Shared]) -> bool {
    let away = |x: Xy| {
        shared_a
            .iter()
            .chain(shared_b)
            .all(|s| !touching(distance(x, s.at)))
    };
    match (*a, *b) {
        (Shape::Line { .. }, Shape::Line { .. }) => false,
        (Shape::Line { a: p0, b: q0 }, Shape::Round { c, arc, .. }) => {
            line_round_beside(p0, q0, shared_a, c, arc.as_ref(), &away)
        }
        (Shape::Round { c, arc, .. }, Shape::Line { a: p0, b: q0 }) => {
            line_round_beside(p0, q0, shared_b, c, arc.as_ref(), &away)
        }
        (Shape::Round { c: c1, arc: a1, .. }, Shape::Round { c: c2, arc: a2, .. }) => {
            let Some(p) = shared_a.first().map(|s| s.at) else {
                return false;
            };
            let d = sub(c2, c1);
            if dot(d, d) == 0.0 {
                return false;
            }
            // The circles' other crossing: the shared point mirrored in the line of
            // centres.
            let w = sub(p, c1);
            let x = sub(add(c1, scale(d, 2.0 * dot(w, d) / dot(d, d))), w);
            away(x) && on_round(c1, a1.as_ref(), x) && on_round(c2, a2.as_ref(), x)
        }
    }
}

/// Whether the line `p0`–`q0`, which shares `shared` with the round, crosses it again away
/// from them: where the line meets the circle again, taking the shared point as on it.
fn line_round_beside(
    p0: Xy,
    q0: Xy,
    shared: &[Shared],
    c: Xy,
    arc: Option<&Arc>,
    away: &dyn Fn(Xy) -> bool,
) -> bool {
    let Some(first) = shared.first() else {
        return false;
    };
    let (p, q) = if first.end == 0 { (p0, q0) } else { (q0, p0) };
    let u = sub(q, p);
    let t = -2.0 * dot(u, sub(p, c)) / dot(u, u);
    let x = add(p, scale(u, t));
    (0.0..=1.0).contains(&t) && away(x) && on_round(c, arc, x)
}

/// A loop as walked: its curves (by place) and whether each is reversed, and its area
/// signed by the walk, as the request measures it.
struct Walked {
    edges: Vec<(usize, bool)>,
    area: f64,
}

/// The loops of a profile whose every point joins two curves: each from its curve with the
/// smallest ID, walked as that curve is drawn; a circle alone.
fn walk(curves: &[ProfileCurve<'_>], at_point: &[Vec<usize>]) -> Result<Vec<Walked>, Error> {
    let broken = || {
        Error::new(
            ErrorCode::Internal,
            "a profile's loop didn't close, though each of its points joins two curves",
        )
    };
    let mut seen = vec![false; curves.len()];
    let mut loops = Vec::new();
    for first in 0..curves.len() {
        if seen[first] {
            continue;
        }
        seen[first] = true;
        let mut edges = vec![(first, false)];
        if let Some([start, mut at]) = curves[first].ends {
            let mut current = first;
            while at != start {
                let next = at_point
                    .get(at)
                    .and_then(|there| there.iter().copied().find(|&c| c != current))
                    .ok_or_else(broken)?;
                if seen[next] {
                    return Err(broken());
                }
                seen[next] = true;
                let [from, to] = curves[next].ends.ok_or_else(broken)?;
                let reversed = from != at;
                at = if reversed { from } else { to };
                edges.push((next, reversed));
                current = next;
            }
        }
        let (area, _) = area_and_perimeter(&sent_loop(curves, &edges).ok_or_else(broken)?);
        loops.push(Walked { edges, area });
    }
    Ok(loops)
}

/// How many times the ray from `p` along +x crosses the loop, by the half-open rule
/// (an end counts as above the ray when it's above `p`); a round is split at its circle's
/// top and bottom so each piece runs one way in y.
fn crossings(p: Xy, curves: &[ProfileCurve<'_>], edges: &[(usize, bool)]) -> usize {
    let mut count = 0;
    let mut piece = |a: Xy, b: Xy, x: &dyn Fn() -> f64| {
        if (a[1] > p[1]) != (b[1] > p[1]) && x() > p[0] {
            count += 1;
        }
    };
    for &(c, _) in edges {
        match curves[c].shape {
            Shape::Line { a, b } => piece(a, b, &|| {
                a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
            }),
            Shape::Round { c, r, arc } => {
                let (top, bottom) = ([c[0], c[1] + r], [c[0], c[1] - r]);
                let dy = p[1] - c[1];
                let half = ((r - dy) * (r + dy)).max(0.0).sqrt();
                let right = || c[0] + half;
                let left = || c[0] - half;
                match arc {
                    None => {
                        piece(bottom, top, &right);
                        piece(top, bottom, &left);
                    }
                    Some(arc) => {
                        // The arc's cuts at the top (π/2 + 2kπ) and bottom (−π/2 + 2kπ)
                        // strictly inside it, from its start to its end.
                        let mut cuts = vec![(arc.from, arc.start)];
                        let mut k = ((arc.from - FRAC_PI_2) / PI).floor() + 1.0;
                        for _ in 0..4 {
                            let angle = FRAC_PI_2 + k * PI;
                            if angle >= arc.to {
                                break;
                            }
                            let at_top = (k as i64).rem_euclid(2) == 0;
                            cuts.push((angle, if at_top { top } else { bottom }));
                            k += 1.0;
                        }
                        cuts.push((arc.to, arc.end));
                        for pair in cuts.windows(2) {
                            if let [(a0, pa), (a1, pb)] = pair {
                                let side = libm::cos((a0 + a1) / 2.0) > 0.0;
                                piece(*pa, *pb, if side { &right } else { &left });
                            }
                        }
                    }
                }
            }
        }
    }
    count
}

/// The loops nested into regions: each loop's depth is how many loops hold one of its
/// points (a point on no other loop: none come within the tolerance of another once no
/// curves meet); a loop at even depth is a region's outer loop, counter-clockwise, and one
/// at odd depth a hole, clockwise, of the loop just around it.
fn regions(
    curves: &[ProfileCurve<'_>],
    ids: &[&ElementId],
    loops: &[Walked],
    plane: Plane,
) -> Result<Profile, Error> {
    let bounds: Vec<[f64; 4]> = loops
        .iter()
        .map(|l| {
            l.edges.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |b, &(c, _)| {
                    let e = curves[c].shape.bounds();
                    [
                        b[0].min(e[0]),
                        b[1].min(e[1]),
                        b[2].max(e[2]),
                        b[3].max(e[3]),
                    ]
                },
            )
        })
        .collect();
    // One point of each loop: its first curve's start, or a circle's (cx + r, cy).
    let probe = |l: &Walked| -> Xy {
        let Some(&(c, _)) = l.edges.first() else {
            return [0.0, 0.0];
        };
        match curves[c].shape {
            Shape::Line { a, .. } => a,
            Shape::Round { arc: Some(arc), .. } => arc.start,
            Shape::Round { c, r, arc: None } => [c[0] + r, c[1]],
        }
    };
    // The loops around each loop, in order.
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); loops.len()];
    for (j, inner) in loops.iter().enumerate() {
        let p = probe(inner);
        for (i, outer) in loops.iter().enumerate() {
            let b = bounds[i];
            if i == j || p[0] < b[0] || p[0] > b[2] || p[1] < b[1] || p[1] > b[3] {
                continue;
            }
            if crossings(p, curves, &outer.edges) % 2 == 1 {
                around[j].push(i);
            }
        }
    }
    let depth: Vec<usize> = around.iter().map(Vec::len).collect();
    // The loop just around each loop, by depth among the loops around it (not every loop at that
    // depth, since separate outlines share depths): loops that don't meet (rule 5 refuses
    // any that do) are disjoint or nested, so the m loops around one are nested in each
    // other, at depths 0 to m - 1, and the innermost is the one at depth m - 1. That rests
    // only on the crossings' parity, so no areas are compared. (Areas would tie only if
    // rounding closed the band, at least 1e-7 wide, between two nested loops; a worst-case
    // bound keeps it open with 6% to spare at a kilometre, and none for a loop that
    // spirals. FreeCAD's Bullseye compares no areas either: a hole goes to the face whose
    // material holds its first vertex.)
    let parent: Vec<Option<usize>> = around
        .iter()
        .zip(&depth)
        .map(|(out, &d)| out.iter().copied().find(|&k| depth[k] + 1 == d))
        .collect();

    let oriented = |l: &Walked, outer: bool| -> Result<Loop, Error> {
        let mut walked = l.edges.clone();
        if (l.area > 0.0) != outer {
            // Walked the other way, still from the first curve.
            if let Some((first, rest)) = walked.split_first_mut() {
                rest.reverse();
                for e in std::iter::once(first).chain(rest.iter_mut()) {
                    e.1 = !e.1;
                }
            }
        }
        // Measured as the request will measure what it's sent.
        let sent = sent_loop(curves, &walked).ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                "a profile's loop has a curve it can't send",
            )
        })?;
        let (area, perimeter) = (loop_area(&sent, outer), area_and_perimeter(&sent).1);
        let edges = walked
            .iter()
            .zip(sent)
            .map(|(&(c, reversed), sent)| {
                let curve = &curves[c];
                Edge {
                    curve: curve.id.clone(),
                    kind: curve.kind,
                    center: curve.center.cloned(),
                    ends: curve.ends.map(|[a, b]| {
                        let (a, b) = (ids[a].clone(), ids[b].clone());
                        if reversed { [b, a] } else { [a, b] }
                    }),
                    reversed,
                    sent,
                }
            })
            .collect();
        Ok(Loop {
            curves: edges,
            area,
            perimeter,
        })
    };

    // Loops are in their first curves' order, which is ID order, so regions and holes
    // come out in it too.
    let mut index_of_region: BTreeMap<usize, usize> = BTreeMap::new();
    let mut regions = Vec::new();
    for (i, l) in loops.iter().enumerate() {
        if depth[i].is_multiple_of(2) {
            index_of_region.insert(i, regions.len());
            regions.push(Region {
                outer: oriented(l, true)?,
                holes: Vec::new(),
                depth: depth[i],
            });
        }
    }
    for (i, l) in loops.iter().enumerate() {
        if depth[i].is_multiple_of(2) {
            continue;
        }
        let region = parent[i]
            .and_then(|k| index_of_region.get(&k))
            .and_then(|&r| regions.get_mut(r))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    "a hole of a profile has no outer loop just around it",
                )
                .with_elements(l.edges.iter().map(|&(c, _)| curves[c].id.to_string()))
            })?;
        region.holes.push(oriented(l, false)?);
    }
    Ok(Profile { plane, regions })
}

impl Profile {
    /// The profile as the kernel's faces request takes it (`KernelRequest::make_faces`):
    /// its plane, and its regions in order, each loop's curves as walked ([`Edge::sent`]),
    /// outer loops counter-clockwise and holes clockwise, exactly as the profile measured
    /// them.
    pub fn faces(&self) -> silico_protocol::FaceRegions {
        silico_protocol::FaceRegions {
            plane: silico_protocol::SketchPlane::from(self.plane),
            regions: self
                .regions
                .iter()
                .map(|r| silico_protocol::Region {
                    outer: r.outer.sent(),
                    holes: r.holes.iter().map(Loop::sent).collect(),
                })
                .collect(),
        }
    }
}
