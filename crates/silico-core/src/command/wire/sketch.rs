//! M5.10's sketch read's wire types (docs/design/m5.0-app.md, section 7, *`sketch`
//! (M5.10)*): its request, read strictly and written back as a client writes it; its result,
//! written field for field ([`ToWire`]: [`SketchRead`], what
//! [`Sessions::sketch`](crate::command::Sessions::sketch) gives), each member as the
//! document's own writer writes it with its state after its fields; and the mirrors its
//! snapshot, `commands/sketch.json`, is generated from.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};

use super::elements::ModelChoiceSchema;
use super::{ErrorSchema, Json, RequestFields, ToWire, close, wire_shape, words};
use crate::command::session::SESSION_ID_PATTERN;
use crate::command::sketch::{Region, SketchRead, state_word};
use crate::command::{ModelChoice, Problems, SessionId};
use crate::document::json::Node;
use crate::document::{Coordinate, ElementId, ElementKind, FeatureId, Form, PROBLEMS_LISTED};
use crate::sketch::{DEPENDENCIES_LISTED, DEPENDENCY_IDS_LISTED};
use crate::sketch_states::{ConstraintState, MemberState, SketchState};

/// A `sketch` request: the session, the model it reads, and the sketch.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchRequest {
    pub session: SessionId,
    pub model: ModelChoice,
    pub sketch: FeatureId,
}

impl SketchRequest {
    /// The request `node` holds, or every reason its shape is refused.
    pub fn read(node: &Node) -> Result<Self, Problems> {
        RequestFields::read(node, |f| {
            let session = f.session();
            let model = f.model();
            let sketch = f.feature("sketch");
            Some(SketchRequest {
                session: session?,
                model: model?,
                sketch: sketch?,
            })
        })
    }
}

impl ToWire for SketchRequest {
    /// The request as a client writes it, every field in the shape's order.
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("session", self.session.to_wire()),
            ("model", self.model.to_wire()),
            ("sketch", self.sketch.to_wire()),
        ])
    }
}

impl ToWire for SketchRead {
    /// `sketch`'s result: the sketch's plane, state, and degrees of freedom; each member as
    /// the document writes it, with its state; the profile's regions, or why there's none;
    /// and why it wasn't solved.
    fn to_wire(&self) -> Json<'_> {
        let frame = silico_protocol::SketchPlane::from(self.plane);
        let states = &self.states;
        // Every member has one: a read whose member had none was refused as a bug
        // (`command::sketch`'s `unstated`), so none is ever written null.
        let state = |kind: ElementKind, id: &ElementId| {
            state_word(states, kind, id)
                .map(|word| ("state", Json::str(word)))
                .into_iter()
                .collect()
        };
        let [points, lines, circles, arcs, constraints] =
            Json::sketch_lists(&self.members, state).map(|(_, list)| list);
        Json::object([
            ("sketch", self.sketch.to_wire()),
            (
                "plane",
                Json::object([
                    ("origin", Json::direction([0.0; 3])),
                    ("x", Json::direction(frame.x_axis())),
                    ("y", Json::direction(frame.y_axis())),
                    ("normal", Json::direction(frame.normal())),
                ]),
            ),
            ("state", Json::str(states.state.as_str())),
            ("dofs", Json::integer(i64::from(states.dofs))),
            ("points", points),
            ("lines", lines),
            ("circles", circles),
            ("arcs", arcs),
            ("constraints", constraints),
            (
                "regions",
                Json::array(self.regions.iter().map(ToWire::to_wire)),
            ),
            (
                "dependencies",
                Json::array(states.dependencies.groups.iter().map(|group| {
                    Json::object([
                        ("constraints", curves(&group.constraints)),
                        ("constraints_total", Json::whole(group.constraints_total)),
                    ])
                })),
            ),
            (
                "dependencies_total",
                Json::whole(states.dependencies.groups_total),
            ),
            (
                "profile_problems",
                Json::array(self.profile_problems.iter().map(ToWire::to_wire)),
            ),
            (
                "profile_problems_total",
                Json::whole(self.profile_problems_total),
            ),
            (
                "solve_problem",
                Json::or_null(states.solve_problem.as_ref(), ToWire::to_wire),
            ),
        ])
    }
}

impl ToWire for Region {
    fn to_wire(&self) -> Json<'_> {
        Json::object([
            ("outer", curves(&self.outer)),
            ("holes", Json::array(self.holes.iter().map(|h| curves(h)))),
        ])
    }
}

/// Curves, or constraints, by their IDs, in order.
fn curves(ids: &[ElementId]) -> Json<'_> {
    Json::array(ids.iter().map(|id| Json::str(id.as_str())))
}

/// `sketch`'s request: a sketch as the model named solved it.
#[derive(JsonSchema)]
#[schemars(rename = "SketchRequest", transform = wire_shape)]
pub struct SketchRequestSchema {
    /// The session's ID, as `open` gave it.
    #[schemars(regex(pattern = SESSION_ID_PATTERN))]
    pub session: String,
    pub model: ModelChoiceSchema,
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
}

/// `sketch`'s result: the sketch as the model's rebuild solved it, for sketch mode to draw
/// and colour.
#[derive(JsonSchema)]
#[schemars(rename = "SketchResult", transform = wire_shape)]
pub struct SketchResultSchema {
    /// The sketch, by its feature's ID.
    pub sketch: FeatureId,
    pub plane: SketchFrameSchema,
    pub state: SketchStateSchema,
    /// The degrees of freedom left, PlaneGCS's number as given: 0 when nothing is free. A
    /// conflict needn't make it negative (two lengths on one free line leave 3); PlaneGCS
    /// gives a negative number where it counts its equations' rows in place of their rank,
    /// as for an arc whose rules alone repeat each other. 0 for an empty sketch.
    pub dofs: i32,
    /// Each point, by ID, as the document writes it, at its solved place.
    pub points: Vec<SketchPointReadSchema>,
    /// Each line, by ID, as the document writes it.
    pub lines: Vec<SketchLineReadSchema>,
    /// Each circle, by ID, as the document writes it, at its solved radius.
    pub circles: Vec<SketchCircleReadSchema>,
    /// Each arc, by ID, as the document writes it.
    pub arcs: Vec<SketchArcReadSchema>,
    /// Each constraint, by ID, as the document writes it, a reference dimension's value as
    /// the solve measured it.
    pub constraints: Vec<SketchConstraintReadSchema>,
    /// The profile's regions, as the profile finds them from the solved curves that aren't
    /// construction geometry (each a face a pad, pocket, revolve, or groove of the sketch
    /// sweeps), by their outer loops' smallest curve IDs: none if the sketch makes no
    /// profile, or wasn't solved.
    pub regions: Vec<SketchRegionSchema>,
    /// Each dependent group of constraints, as the solver's diagnosis finds them: the
    /// constraint IDs of one linear dependency among the sketch's equations, sorted, the
    /// groups sorted. Removing any one constraint of a group resolves it; which is the
    /// user's choice. A driving dimension in any group reads `redundant` and over-defines,
    /// whichever constraint the solver dropped. The first 100 groups are listed, each its
    /// first 100 constraints. Empty for an empty sketch, and for one whose constraints the
    /// solver never took (geometry that can't be held, before any constraint).
    #[schemars(length(max = DEPENDENCIES_LISTED))]
    pub dependencies: Vec<SketchDependencySchema>,
    /// How many dependent groups there are, listed or not.
    pub dependencies_total: u64,
    /// Why the solved sketch makes no profile, each as an error (the first 100, in the
    /// order they're checked): none if it makes one, or wasn't solved (`solve_problem` says
    /// why).
    #[schemars(length(max = PROBLEMS_LISTED))]
    pub profile_problems: Vec<ErrorSchema>,
    /// How many reasons there are, listed or not.
    pub profile_problems_total: u64,
    /// Why the sketch wasn't solved, as `check` reports it: a conflict (`over_constrained`),
    /// no solution (`solve_failed`), or geometry that can't be held (`invalid_result`,
    /// `invalid_input`). Null when it solved.
    pub solve_problem: Option<ErrorSchema>,
}

/// The sketch's plane in global coordinates: its origin, and the directions of its x, its y,
/// and its normal (x × y, about which counter-clockwise turns), each a unit vector.
#[derive(JsonSchema)]
#[schemars(rename = "SketchFrame", transform = wire_shape)]
pub struct SketchFrameSchema {
    /// The sketch's (0, 0), in global coordinates (mm).
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub normal: [f64; 3],
}

/// A point, as the document writes it, at its solved place, with its state.
#[derive(JsonSchema)]
#[schemars(rename = "SketchPointRead", transform = wire_shape)]
pub struct SketchPointReadSchema {
    pub id: ElementId,
    /// Its x, as solved (as stored, if the sketch wasn't solved).
    pub x: Coordinate,
    /// Its y, as solved (as stored, if the sketch wasn't solved).
    pub y: Coordinate,
    pub construction: bool,
    pub state: MemberStateSchema,
}

/// A line, as the document writes it, with its state.
#[derive(JsonSchema)]
#[schemars(rename = "SketchLineRead", transform = wire_shape)]
pub struct SketchLineReadSchema {
    pub id: ElementId,
    pub from: ElementId,
    pub to: ElementId,
    pub construction: bool,
    pub state: MemberStateSchema,
}

/// A circle, as the document writes it, at its solved radius, with its state.
#[derive(JsonSchema)]
#[schemars(rename = "SketchCircleRead", transform = wire_shape)]
pub struct SketchCircleReadSchema {
    pub id: ElementId,
    pub center: ElementId,
    /// Its radius, as solved (as stored, if the sketch wasn't solved).
    pub radius: Coordinate,
    pub construction: bool,
    pub state: MemberStateSchema,
}

/// An arc, as the document writes it, with its state.
#[derive(JsonSchema)]
#[schemars(rename = "SketchArcRead", transform = wire_shape)]
pub struct SketchArcReadSchema {
    pub id: ElementId,
    pub center: ElementId,
    pub start: ElementId,
    pub end: ElementId,
    pub construction: bool,
    pub state: MemberStateSchema,
}

/// A region of the profile: its outer loop's curves and each hole's, by ID.
#[derive(JsonSchema)]
#[schemars(rename = "SketchRegion", transform = wire_shape)]
pub struct SketchRegionSchema {
    /// The outer loop's curves, in the order it walks them, counter-clockwise.
    pub outer: Vec<ElementId>,
    /// Each hole's curves, in the order it walks them, clockwise; the holes by their
    /// smallest curve IDs.
    pub holes: Vec<Vec<ElementId>>,
}

/// One dependent group: its constraints' IDs, sorted, and how many it has, as an error's
/// `elements` and `elements_total`.
#[derive(JsonSchema)]
#[schemars(rename = "SketchDependency", transform = wire_shape)]
pub struct SketchDependencySchema {
    /// The group's constraints, by ID, sorted: the first 100.
    #[schemars(length(min = 1, max = DEPENDENCY_IDS_LISTED))]
    pub constraints: Vec<ElementId>,
    /// How many constraints the group has, listed or not.
    pub constraints_total: u64,
}

/// A constraint, as the document writes it (its ID, its type, and its form's fields, a
/// reference dimension's value as the solve measured it), with its state.
pub struct SketchConstraintReadSchema;

impl JsonSchema for SketchConstraintReadSchema {
    fn schema_name() -> Cow<'static, str> {
        "SketchConstraintRead".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let id = generator.subschema_for::<ElementId>();
        let state = generator.subschema_for::<ConstraintStateSchema>();
        // Each of the document's forms, its own object, with the constraint's ID and its
        // state beside its fields, and closed to them.
        let mut schema = <Form as JsonSchema>::json_schema(generator);
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "description".to_owned(),
                serde_json::Value::String(
                    "A constraint, as the document writes it (its ID, its type, and its \
                     form's fields, a reference dimension's value as the solve measured it), \
                     with its state."
                        .to_owned(),
                ),
            );
            if let Some(serde_json::Value::Array(forms)) = object.get_mut("oneOf") {
                for form in forms {
                    if let Some(form) = form.as_object_mut() {
                        if let Some(serde_json::Value::Object(properties)) =
                            form.get_mut("properties")
                        {
                            properties.insert("id".to_owned(), id.clone().into());
                            properties.insert("state".to_owned(), state.clone().into());
                        }
                        close(form);
                    }
                }
            }
        }
        schema
    }
}

/// A sketch's state.
pub struct SketchStateSchema;

impl JsonSchema for SketchStateSchema {
    fn schema_name() -> Cow<'static, str> {
        "SketchState".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "The sketch's state, as SolidWorks names it: under_defined (solved, something \
             free), fully_defined (solved, nothing free; or empty), over_defined (constraints \
             in conflict, so not solved; or solved, with a driving dimension whose value the \
             others fix), no_solution (no algorithm placed it), or invalid (solved only to \
             geometry that can't be built or held).",
            SketchState::ALL.map(SketchState::as_str),
        )
    }
}

/// A point's or a curve's state.
pub struct MemberStateSchema;

impl JsonSchema for MemberStateSchema {
    fn schema_name() -> Cow<'static, str> {
        "MemberState".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A point's or a curve's state: over_defined when a constraint that over-defines \
             governs it (a point when such a constraint reads its x or y; a curve when one \
             names it, not taking its points' state); otherwise under_defined when any of its \
             values (a curve's own, or its points') is free; otherwise fully_defined.",
            MemberState::ALL.map(MemberState::as_str),
        )
    }
}

/// A constraint's state.
pub struct ConstraintStateSchema;

impl JsonSchema for ConstraintStateSchema {
    fn schema_name() -> Cow<'static, str> {
        "ConstraintState".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        words(
            "A constraint's state: conflicting (in conflict with others, so the sketch \
             isn't solved); redundant (its equations repeat others': removing it, or one of \
             those it repeats, resolves it); partially_redundant (some of them do); \
             satisfied (on a solved sketch, independent of the others and met, a reference \
             dimension always); or unsolved (on a sketch that wasn't solved, any not \
             flagged).",
            ConstraintState::ALL.map(ConstraintState::as_str),
        )
    }
}
