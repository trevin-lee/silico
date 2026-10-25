//! M4.4's element reads, `elements` and `query` (docs/design/m4.1-command-api.md, section 2's
//! *Element entries* and section 7): a model's solids' faces, edges, and vertices as entries,
//! each named by its element map and measured by its rebuild's description
//! ([`Model::description`]), so a read asks the kernel nothing. A model builds each solid's
//! entries once, the first time they're read ([`Model::entries`]), and each call copies only
//! what it returns. These are the commands' answers from a model; a session runs them on the
//! model a read names (decision 33).

use super::session::faulted;
use super::{ModelChoice, Problems, Revision, Running, SessionId, Sessions};
use crate::document::{Angle, FeatureId, Length, Measure};
pub use crate::elements::{Alias, AxisToken, ElementType, Entry, EntryAxis};
use crate::kernel::{Clients, ElementKind, ShapeSummary};
use crate::naming::{Maker, Name};
use crate::recompute::{Built, FeatureOutcome, Model};
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `elements` (M4.4): `limit` entries of the model `model` names (decision 33), of the
    /// feature `feature`'s solid or of every body, after `after`'s entry ([`list`]). A read
    /// of a published model asks the kernel nothing: its solids were described as it was
    /// rebuilt.
    pub fn elements(
        &self,
        session: &SessionId,
        model: ModelChoice,
        feature: Option<FeatureId>,
        after: Option<Cursor>,
        limit: u32,
    ) -> Running<Listed> {
        self.read_model(session, "elements", model, move |read| {
            list(
                read.model(),
                read.model_read().revision,
                feature.as_ref(),
                after.as_ref(),
                limit,
            )
        })
    }

    /// `query` (M4.4): the entries of the model `model` names that match `filter`
    /// ([`query`]).
    pub fn query(
        &self,
        session: &SessionId,
        model: ModelChoice,
        filter: Query,
    ) -> Running<Queried> {
        self.read_model(session, "query", model, move |read| {
            query(read.model(), &filter)
        })
    }
}

/// The most entries one `elements` or `query` call returns, and the most a `limit` asks for.
pub const ENTRIES_AT_MOST: u32 = 1000;

/// The most ranges a `query`'s `measures` holds: as many as there are measures, so a
/// request can't make a call check a million ranges against every element, or refuse it
/// for a million reasons.
pub const MEASURES_AT_MOST: usize = MeasureName::ALL.len();

/// Each of the feature `id`'s solid's elements as an entry, in the element list's order:
/// faces, then edges, then vertices, each by name, byte by byte. Refused, naming `feature`,
/// for a feature the document doesn't have, a sketch, or one that didn't build or couldn't
/// be described (its failure, as `check` reports it).
pub fn entries<'m>(model: &'m Model, id: &FeatureId) -> Result<&'m [Entry], Problems> {
    solid(model, id, true)
}

/// The feature `id`'s solid's entries, or why they can't be had: naming the request's
/// `feature` if `asked` (the request named it), and otherwise only the feature (a body of
/// a listing of every body, its `feature` null).
fn solid<'m>(model: &'m Model, id: &FeatureId, asked: bool) -> Result<&'m [Entry], Problems> {
    let built = built(model, id, asked)?;
    match model.entries(id) {
        Some(Ok(entries)) => Ok(entries),
        Some(Err(fault)) => Err(fault.clone().into()),
        // Not described. Why: its description refused or failed, or, before that, its
        // summary.
        None => Err(named(
            asked,
            match built
                .why_undescribed()
                .or(built.measure_refused())
                .or(model.unmeasured(id))
            {
                Some(refused) => refused.clone(),
                None => Error::new(
                    ErrorCode::KernelFailed,
                    format!("{id}'s solid couldn't be described, so its elements aren't known"),
                )
                .with_suggestion("check the document again: its rebuild describes it again"),
            }
            .with_feature(id.as_str()),
        )
        .into()),
    }
}

/// `error`, naming the request's `feature` if `asked`.
fn named(asked: bool, error: Error) -> Error {
    if asked {
        error.with_parameter("feature")
    } else {
        error
    }
}

/// The feature `id`'s built solid, or why it has none: a feature the document doesn't
/// have, a sketch, or one that failed (its failure, as `check` reports it) or was blocked;
/// naming the request's `feature` if `asked`, as [`entries`] names it. Every read of a
/// feature's solid (`elements`, `query`, `mass_properties`) refuses one alike.
pub(crate) fn built<'m>(
    model: &'m Model,
    id: &FeatureId,
    asked: bool,
) -> Result<&'m Built, Problems> {
    let named = |error: Error| named(asked, error);
    match model.outcome(id) {
        None => Err(named(
            Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {id}"),
            )
            .with_suggestion("name a feature of the document, by its ID"),
        )
        .into()),
        Some(FeatureOutcome::Sketch(_)) => Err(named(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{id} is a sketch, which makes no solid"),
            )
            .with_feature(id.as_str())
            .with_suggestion("name a feature that makes a solid"),
        )
        .into()),
        Some(FeatureOutcome::Failed(error)) => Err(error.clone().into()),
        Some(FeatureOutcome::Broken { errors, total }) => Err(Problems::with_unlisted(
            errors.clone(),
            total.saturating_sub(errors.len() as u64),
        )),
        Some(FeatureOutcome::Blocked { on }) => Err(named(
            Error::new(
                ErrorCode::InvalidInput,
                format!("{id} didn't build, since it uses {on}, which failed"),
            )
            .with_feature(id.as_str())
            .with_suggestion(format!("mend {on} first: `check` says why it failed")),
        )
        .into()),
        Some(FeatureOutcome::Built { built, .. }) => Ok(built),
    }
}

/// The feature `id`'s built solid's summary, or why it has none, as `check` says it: the
/// kernel's refusal of its values (kept with the solid, or the model's
/// [`unmeasured`](Model::unmeasured)), or a worker's failure to measure it (the model's
/// alone); naming the
/// request's `feature` if `asked`, as [`built`] does. Every read of a solid's measures
/// (`mass_properties`) refuses one alike.
pub(crate) fn summarized<'m>(
    model: &'m Model,
    id: &FeatureId,
    asked: bool,
) -> Result<&'m ShapeSummary, Problems> {
    let built = built(model, id, asked)?;
    if let Some(summary) = built.summary() {
        return Ok(summary);
    }
    let why = match built.measure_refused().or(model.unmeasured(id)) {
        Some(why) => why.clone(),
        // A recompute measures every solid it builds, or keeps why it couldn't.
        None => Error::new(
            ErrorCode::Internal,
            format!("{id}'s solid built, but it's neither measured nor said why not"),
        )
        .with_suggestion(crate::worker::SILICO_BUG),
    };
    Err(named(asked, why.with_feature(id.as_str())).into())
}

/// The entries a call reads: the feature `feature`'s solid's, or, for none, each body's
/// (the solids no later feature consumed), in timeline order; every reason one can't be
/// read added to `refused`.
fn sources<'m>(
    model: &'m Model,
    feature: Option<&FeatureId>,
    refused: &mut Problems,
) -> Vec<&'m [Entry]> {
    let mut sources = Vec::new();
    let read =
        |id: &FeatureId, asked: bool, sources: &mut Vec<&'m [Entry]>, refused: &mut Problems| {
            match solid(model, id, asked) {
                Ok(entries) => sources.push(entries),
                Err(problems) => refused.absorb(problems),
            }
        };
    match feature {
        Some(id) => read(id, true, &mut sources, refused),
        None => {
            for id in model.bodies() {
                read(id, false, &mut sources, refused);
            }
        }
    }
    sources
}

/// Where a listing of elements goes on from: the revision it's of, and the last entry
/// listed, by its solid's feature and its own name.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub revision: Revision,
    pub feature: FeatureId,
    pub name: String,
}

/// A page of `elements`: its entries, how many there are in all, and where the next page
/// starts (`None` after the last, and for a listing of every body of a model that holds a
/// kernel fault).
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub entries: Vec<Entry>,
    pub total: u64,
    pub next: Option<Cursor>,
}

/// `elements` on `model`, of `revision`: `limit` entries of the feature `feature`'s solid,
/// or, for none, of every body (the solids no later feature consumed, in timeline order),
/// after `after`'s entry. Every reason it's refused is given, in the request's order: the
/// feature's (or each body's), the cursor's, then the limit's.
///
/// A model that holds a kernel fault (a worker that crashed or hung) may not be the
/// revision's next read: a session rebuilds it (`Session::model`), and a feature that
/// failed for the fault consumes nothing, so its inputs are bodies until it builds. So a
/// listing of its every body gives no cursor, and a cursor on one is a `conflict`: pages of
/// two models of one revision are never joined (until a revision's fault is its outcome,
/// decision 32). A feature's own listing pages on: its elements don't change.
pub fn list(
    model: &Model,
    revision: Revision,
    feature: Option<&FeatureId>,
    after: Option<&Cursor>,
    limit: u32,
) -> Result<Listed, Problems> {
    let mut refused = Problems::none();
    let sources = sources(model, feature, &mut refused);
    // Only a listing of every body changes between two models of a revision (which bodies
    // there are follows from which features built); a feature's own listing doesn't.
    let whole = feature.is_none();
    let resume = after.and_then(|cursor| {
        cursor_name(model, revision, cursor, whole, &mut refused).map(|name| (cursor, name))
    });
    if let Some(error) = limit_refused(limit) {
        refused.push(error);
    }
    refused.into_result()?;
    let total: usize = sources.iter().map(|entries| entries.len()).sum();
    let (body, from) = match resume {
        None => (0, 0),
        Some((cursor, name)) => match found(&sources, &cursor.feature, &name) {
            Some((body, at)) => (body, at + 1),
            None => {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "the cursor names {} of {}, which this listing doesn't hold",
                        cursor.name, cursor.feature
                    ),
                )
                .with_parameter("after")
                .with_suggestion(
                    "start again with after set to null, or pass the last call's next as it was",
                )
                .into());
            }
        },
    };
    let limit = limit as usize;
    let mut entries: Vec<Entry> = Vec::with_capacity(limit.min(total));
    let mut skip = from;
    for source in sources.get(body..).unwrap_or_default() {
        let rest = source.get(skip..).unwrap_or_default();
        skip = 0;
        let take = limit.saturating_sub(entries.len()).min(rest.len());
        entries.extend_from_slice(rest.get(..take).unwrap_or_default());
        if entries.len() == limit {
            break;
        }
    }
    let before: usize = sources.iter().take(body).map(|entries| entries.len()).sum();
    let end = before + from + entries.len();
    // Whether it's faulted is asked only of a whole listing with more to come: it's a check.
    let next = (end < total && !(whole && faulted(model)))
        .then(|| entries.last())
        .flatten()
        .map(|last| Cursor {
            revision,
            feature: last.feature.clone(),
            name: last.name.to_string(),
        });
    Ok(Listed {
        entries,
        total: total as u64,
        next,
    })
}

/// The name a cursor goes on from, read once, if the cursor is of the model read: otherwise
/// why it isn't, added to `refused`, naming `after`.
fn cursor_name(
    model: &Model,
    revision: Revision,
    cursor: &Cursor,
    whole: bool,
    refused: &mut Problems,
) -> Option<Name> {
    let conflict = |message: String, suggestion: &str| {
        Error::new(ErrorCode::Conflict, message)
            .with_parameter("after")
            .with_suggestion(suggestion)
    };
    if cursor.revision != revision {
        refused.push(conflict(
            format!(
                "the cursor is of revision {}, and the model read is of revision {}",
                cursor.revision.number(),
                revision.number()
            ),
            "start again with after set to null",
        ));
        return None;
    }
    if whole && faulted(model) {
        refused.push(conflict(
            format!(
                "the model read of revision {} holds a kernel fault, so it may not be the \
                 model the cursor's pages were listed from",
                revision.number()
            ),
            "check again, then list from the start",
        ));
        return None;
    }
    let makers = |id: &FeatureId| {
        model
            .document()
            .features
            .iter()
            .find(|f| f.id == *id)
            .map(|f| Maker::of(&f.operation))
    };
    match Name::parse(&cursor.name, &makers) {
        Ok(name) => Some(name),
        Err(error) => {
            refused.push(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "the cursor's name, {}, isn't an element's name: {error}",
                        crate::document::json::shown(&cursor.name)
                    ),
                )
                .with_parameter("after")
                .with_suggestion(
                    "start again with after set to null, or pass the last call's next as it was",
                ),
            );
            None
        }
    }
}

/// Where the entry named `name` of the feature `feature`'s solid is among `sources`: which
/// of them, and where in it. Found by halves, on the order a solid's entries are in (by
/// kind, then by name, byte by byte), so only the names it passes on the way are written
/// out.
fn found(sources: &[&[Entry]], feature: &FeatureId, name: &Name) -> Option<(usize, usize)> {
    let key = (rank(name.kind()), name.to_string());
    sources.iter().enumerate().find_map(|(i, entries)| {
        // A solid's entries are all its feature's.
        if entries.first()?.feature != *feature {
            return None;
        }
        let at = entries
            .binary_search_by(|e| (rank(e.kind), e.name.to_string()).cmp(&key))
            .ok()?;
        Some((i, at))
    })
}

/// A kind's place in the element list's order: faces, edges, vertices.
fn rank(kind: ElementKind) -> u8 {
    match kind {
        ElementKind::Face => 0,
        ElementKind::Edge => 1,
        ElementKind::Vertex => 2,
        ElementKind::Solid => 3,
    }
}

/// The refusal of a `limit` past what a call returns, if it is.
fn limit_refused(limit: u32) -> Option<Error> {
    (limit == 0 || limit > ENTRIES_AT_MOST).then(|| {
        Error::new(
            ErrorCode::InvalidInput,
            format!("limit must be from 1 to {ENTRIES_AT_MOST}, got {limit}"),
        )
        .with_parameter("limit")
        .with_suggestion(format!(
            "list in pages of at most {ENTRIES_AT_MOST}, with after set to the last call's next"
        ))
    })
}

/// A measure an entry has, which `query` filters and orders by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureName {
    Area,
    Length,
    Radius,
    Diameter,
    MajorRadius,
    MinorRadius,
    HalfAngle,
}

impl MeasureName {
    pub const ALL: [MeasureName; 7] = [
        MeasureName::Area,
        MeasureName::Length,
        MeasureName::Radius,
        MeasureName::Diameter,
        MeasureName::MajorRadius,
        MeasureName::MinorRadius,
        MeasureName::HalfAngle,
    ];

    pub fn word(self) -> &'static str {
        match self {
            MeasureName::Area => "area",
            MeasureName::Length => "length",
            MeasureName::Radius => "radius",
            MeasureName::Diameter => "diameter",
            MeasureName::MajorRadius => "major_radius",
            MeasureName::MinorRadius => "minor_radius",
            MeasureName::HalfAngle => "half_angle",
        }
    }

    /// Whether an element of `kind` can have it: a face its area, a cylinder's or sphere's
    /// radius, a cylinder's diameter, a torus's radii, and a cone's half-angle; an edge its
    /// length, and a circle's or arc's radius and diameter; a vertex none.
    pub fn is_of(self, kind: ElementKind) -> bool {
        match kind {
            ElementKind::Face => self != MeasureName::Length,
            ElementKind::Edge => matches!(
                self,
                MeasureName::Length | MeasureName::Radius | MeasureName::Diameter
            ),
            ElementKind::Vertex | ElementKind::Solid => false,
        }
    }

    /// The measure's quantity, as written: an area, a length, or an angle.
    pub(crate) fn quantity(self) -> QuantityKind {
        match self {
            MeasureName::Area => QuantityKind::Area,
            MeasureName::HalfAngle => QuantityKind::Angle,
            MeasureName::Length
            | MeasureName::Radius
            | MeasureName::Diameter
            | MeasureName::MajorRadius
            | MeasureName::MinorRadius => QuantityKind::Length,
        }
    }

    /// The entry's value of it, as written, if it has it.
    fn of(self, entry: &Entry) -> Option<Quantity> {
        match self {
            MeasureName::Area => entry.area.map(Quantity::Area),
            MeasureName::Length => entry.length.map(Quantity::Length),
            MeasureName::Radius => entry.radius.map(Quantity::Length),
            MeasureName::Diameter => entry.diameter.map(Quantity::Length),
            MeasureName::MajorRadius => entry.major_radius.map(Quantity::Length),
            MeasureName::MinorRadius => entry.minor_radius.map(Quantity::Length),
            MeasureName::HalfAngle => entry.half_angle.map(Quantity::Angle),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuantityKind {
    Area,
    Length,
    Angle,
}

/// A measure's value, as written: compared exactly, so values equal as written are equal
/// (decision 19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    Area(Measure),
    Length(Length),
    Angle(Angle),
}

impl Quantity {
    fn kind(self) -> QuantityKind {
        match self {
            Quantity::Area(_) => QuantityKind::Area,
            Quantity::Length(_) => QuantityKind::Length,
            Quantity::Angle(_) => QuantityKind::Angle,
        }
    }

    /// Its value in its unit's least step (millionths of a mm² or mm³, nanometres,
    /// nanodegrees), for comparing two of one kind.
    fn steps(self) -> i128 {
        match self {
            Quantity::Area(m) => m.millionths(),
            Quantity::Length(l) => i128::from(l.nm()),
            Quantity::Angle(a) => i128::from(a.ndeg()),
        }
    }
}

/// `query`'s filter: a kind, and optionally a type, stability, a direction a planar face
/// faces, ranges of measures, and an order (plan decision 4).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub feature: Option<FeatureId>,
    pub kind: ElementKind,
    pub r#type: Option<ElementType>,
    /// `Some(true)`: elements with a stable name (their own or an alias); `Some(false)`:
    /// those with none.
    pub stable: Option<bool>,
    pub facing: Option<Facing>,
    pub measures: Vec<Range>,
    pub order: Option<Order>,
    pub limit: u32,
}

impl Query {
    /// A query of every body's elements of `kind`, nothing else asked, `limit` at most.
    pub fn of(kind: ElementKind, limit: u32) -> Self {
        Query {
            feature: None,
            kind,
            r#type: None,
            stable: None,
            facing: None,
            measures: Vec::new(),
            order: None,
            limit,
        }
    }
}

/// Faces facing `direction`: whose outward normal is at most `within` from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Facing {
    pub direction: [f64; 3],
    pub within: Angle,
}

/// A measure from `at_least` to `at_most`, either end open, both included.
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub measure: MeasureName,
    pub at_least: Option<Quantity>,
    pub at_most: Option<Quantity>,
}

/// Entries by a measure, the smallest first unless `descending`. Only entries that have
/// the measure are ordered by it, so ordering by one keeps only those.
#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub by: MeasureName,
    pub descending: bool,
}

/// `query`'s answer: the entries, how many matched the filter before the limit, and the
/// tie at the limit's cut, if there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct Queried {
    pub entries: Vec<Entry>,
    pub matched: u64,
    pub tied: Option<Tied>,
}

/// Entries equal by the order's measure, as written, across the limit's cut: how many, and
/// whether they're all returned (past `limit`), or none of them are.
#[derive(Debug, Clone, PartialEq)]
pub struct Tied {
    pub count: u64,
    pub included: bool,
}

/// `query` on `model`: the entries that match `query`, in its order (or `elements`'s), the
/// first `limit` of them. A tie at the cut is never broken: it's returned whole, past
/// `limit`, if it ends within [`ENTRIES_AT_MOST`] entries, and otherwise none of it is
/// (decision 4). Every reason the query is refused is given, in the order of its fields.
pub fn query(model: &Model, query: &Query) -> Result<Queried, Problems> {
    let mut refused = Problems::none();
    let sources = sources(model, query.feature.as_ref(), &mut refused);
    query_refused(query, &mut refused);
    refused.into_result()?;
    let matched: Vec<&Entry> = sources
        .iter()
        .flat_map(|entries| entries.iter())
        .filter(|entry| matches(entry, query))
        .collect();
    let Some(order) = &query.order else {
        return Ok(Queried {
            matched: matched.len() as u64,
            entries: matched
                .into_iter()
                .take(query.limit as usize)
                .cloned()
                .collect(),
            tied: None,
        });
    };
    let mut ordered: Vec<(i128, &Entry)> = matched
        .into_iter()
        .filter_map(|entry| order.by.of(entry).map(|v| (v.steps(), entry)))
        .collect();
    // Stable, so entries equal by the measure keep `elements`'s order.
    ordered.sort_by(|(a, _), (b, _)| {
        let by = a.cmp(b);
        if order.descending { by.reverse() } else { by }
    });
    let count = ordered.len() as u64;
    let limit = query.limit as usize;
    let cut = match (ordered.get(limit.wrapping_sub(1)), ordered.get(limit)) {
        (Some((last, _)), Some((next, _))) if last == next => Some(*last),
        _ => None,
    };
    let (keep, tied) = match cut {
        None => (limit.min(ordered.len()), None),
        Some(value) => {
            let first = ordered.iter().position(|(v, _)| *v == value).unwrap_or(0);
            let end = first
                + ordered
                    .get(first..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|(v, _)| *v == value)
                    .count();
            let fits = end <= ENTRIES_AT_MOST as usize;
            (
                if fits { end } else { first },
                Some(Tied {
                    count: (end - first) as u64,
                    included: fits,
                }),
            )
        }
    };
    Ok(Queried {
        entries: ordered
            .into_iter()
            .take(keep)
            .map(|(_, entry)| entry.clone())
            .collect(),
        matched: count,
        tied,
    })
}

/// Whether `entry` matches `query`'s filter (its order's measure aside).
fn matches(entry: &Entry, query: &Query) -> bool {
    entry.kind == query.kind
        && query.r#type.is_none_or(|t| entry.r#type == t)
        && query.stable.is_none_or(|stable| {
            (entry.stable || entry.aliases.iter().any(|alias| alias.stable)) == stable
        })
        && query
            .facing
            .as_ref()
            .is_none_or(|facing| entry.normal.is_some_and(|normal| faces(normal, facing)))
        && query.measures.iter().all(|range| {
            range.measure.of(entry).is_some_and(|value| {
                range
                    .at_least
                    .is_none_or(|low| value.steps() >= low.steps())
                    && range
                        .at_most
                        .is_none_or(|high| value.steps() <= high.steps())
            })
        })
}

/// Whether a face of outward normal `normal` faces `facing.direction` within its angle: the
/// angle between them, as written (to the nanodegree), at most `within`.
fn faces(normal: [f64; 3], facing: &Facing) -> bool {
    // Scaled by its largest component, so a direction of any size squares without
    // overflowing or losing digits to underflow: the angle is its unit direction's.
    let largest = facing
        .direction
        .iter()
        .fold(0.0f64, |largest, c| largest.max(c.abs()));
    let d = facing.direction.map(|c| c / largest);
    let cross = [
        normal[1] * d[2] - normal[2] * d[1],
        normal[2] * d[0] - normal[0] * d[2],
        normal[0] * d[1] - normal[1] * d[0],
    ];
    let sin = cross.iter().map(|c| c * c).sum::<f64>().sqrt();
    let cos: f64 = normal.iter().zip(d).map(|(n, d)| n * d).sum();
    Angle::rounded(libm::atan2(sin, cos).to_degrees()).is_some_and(|angle| angle <= facing.within)
}

/// Every reason `query` is refused but its feature's, in the order its fields come, added
/// to `refused`: for a kind of solid, which no element is, only those the kind doesn't
/// decide; for more ranges than [`MEASURES_AT_MOST`], that, and none of theirs.
fn query_refused(query: &Query, refused: &mut Problems) {
    let invalid = |message: String, parameter: &str| {
        Error::new(ErrorCode::InvalidInput, message).with_parameter(parameter)
    };
    let solid = query.kind == ElementKind::Solid;
    if solid {
        refused.push(
            invalid(
                "kind must be face, edge, or vertex, got solid".to_owned(),
                "kind",
            )
            .with_suggestion("give one of the kinds")
            .with_candidates(["face", "edge", "vertex"].map(str::to_owned)),
        );
    }
    let kind = query.kind.word();
    let a_kind = query.kind.a_word();
    if let Some(t) = query.r#type
        && !solid
        && !t.is_of(query.kind)
    {
        refused.push(
            invalid(format!("{a_kind} is never of type {}", t.word()), "type")
                .with_suggestion(format!("give a type {a_kind} can have"))
                .with_candidates(
                    ElementType::ALL
                        .into_iter()
                        .filter(|t| t.is_of(query.kind))
                        .map(|t| t.word().to_owned()),
                ),
        );
    }
    if let Some(facing) = &query.facing {
        if solid {
            // No element is a solid, so whether it could face anything isn't asked.
        } else if query.kind != ElementKind::Face {
            refused.push(
                invalid(
                    format!("only planar faces face a direction, and kind is {kind}"),
                    "facing",
                )
                .with_suggestion("query faces of type plane to filter by direction"),
            );
        } else if query.r#type.is_some_and(|t| t != ElementType::Plane) {
            refused.push(
                invalid(
                    "only planar faces face a direction, and type isn't plane".to_owned(),
                    "facing",
                )
                .with_suggestion(
                    "query faces of type plane, or of no type, to filter by direction",
                ),
            );
        }
        let d = facing.direction;
        if !(d.iter().all(|c| c.is_finite()) && d.iter().any(|c| *c != 0.0)) {
            refused.push(
                invalid(
                    format!(
                        "facing.direction must be a direction, got [{}, {}, {}]",
                        d[0], d[1], d[2]
                    ),
                    "facing.direction",
                )
                .with_suggestion("give a direction that isn't zero"),
            );
        }
        if facing.within < Angle::from_ndeg(0) || facing.within > Angle::from_ndeg(180_000_000_000)
        {
            refused.push(
                invalid(
                    format!(
                        "facing.within must be from 0 to 180 degrees, got {}",
                        facing.within
                    ),
                    "facing.within",
                )
                .with_suggestion(
                    "give the angle a face's normal may be off the direction, in degrees",
                ),
            );
        }
    }
    if query.measures.len() > MEASURES_AT_MOST {
        refused.push(too_many_ranges(query.measures.len()));
    } else {
        for (i, range) in query.measures.iter().enumerate() {
            range_refused(query.kind, solid, i, range, refused);
        }
    }
    if let Some(order) = &query.order
        && !solid
        && !order.by.is_of(query.kind)
    {
        refused.push(
            invalid(format!("{a_kind} has no {}", order.by.word()), "order.by")
                .with_suggestion(format!("order by a measure {a_kind} has"))
                .with_candidates(measures_of(query.kind)),
        );
    }
    if let Some(error) = limit_refused(query.limit) {
        refused.push(error);
    }
}

/// The refusal of a query's `count` ranges, more than [`MEASURES_AT_MOST`]: one reason, its
/// ranges not checked.
pub(crate) fn too_many_ranges(count: usize) -> Error {
    Error::new(
        ErrorCode::InvalidInput,
        format!(
            "measures holds at most {MEASURES_AT_MOST} ranges, as many as there are measures, \
             got {count}"
        ),
    )
    .with_parameter("measures")
    .with_suggestion("give each measure one range at most, from at_least to at_most")
}

/// Every reason the `i`th range, of a query of `kind` (a `solid`, which decides nothing),
/// is refused, added to `refused`: each made only if it's listed.
fn range_refused(kind: ElementKind, solid: bool, i: usize, range: &Range, refused: &mut Problems) {
    let at = |field: &str| format!("measures.{i}.{field}");
    let a_kind = kind.a_word();
    if !solid && !range.measure.is_of(kind) {
        refused.push_with(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!("{a_kind} has no {}", range.measure.word()),
            )
            .with_parameter(at("measure"))
            .with_suggestion(format!("give a measure {a_kind} has"))
            .with_candidates(measures_of(kind))
        });
    }
    let quantity = range.measure.quantity();
    for (field, bound) in [("at_least", range.at_least), ("at_most", range.at_most)] {
        if let Some(bound) = bound
            && bound.kind() != quantity
        {
            refused.push_with(|| {
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{} is {}, and {field} isn't",
                        range.measure.word(),
                        quantity_word(quantity)
                    ),
                )
                .with_parameter(at(field))
                .with_suggestion(format!("give {field} as {}", quantity_word(quantity)))
            });
        }
    }
    if let (Some(low), Some(high)) = (range.at_least, range.at_most)
        && low.kind() == high.kind()
        && low.steps() > high.steps()
    {
        refused.push_with(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!("measures.{i} is empty: at_least is more than at_most"),
            )
            .with_parameter(at("at_least"))
            .with_suggestion("give at_least no more than at_most")
        });
    }
}

fn quantity_word(kind: QuantityKind) -> &'static str {
    match kind {
        QuantityKind::Area => "an area",
        QuantityKind::Length => "a length",
        QuantityKind::Angle => "an angle",
    }
}

/// The measures an element of `kind` can have, by name.
fn measures_of(kind: ElementKind) -> impl Iterator<Item = String> {
    MeasureName::ALL
        .into_iter()
        .filter(move |m| m.is_of(kind))
        .map(|m| m.word().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{
        Description, ElementCounts, ElementFate, FaceDescription, History, InputHistory, Surface,
        Topology,
    };
    use crate::naming::{ElementMap, OpCode};

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    #[test]
    fn a_tilted_face_faces_a_direction_by_the_whole_angle_between_them() {
        // Outward normal (0.48, 0.6, 0.64), a unit vector with every component its own, and
        // the direction (1, 2, 2): n · d / |d| = 2.96 / 3, so the angle between them is
        // 9.366788884 degrees (atan2 of |n × d| = |(-0.08, -0.32, 0.36)| / 3 over that), as
        // written to the nanodegree. A face at exactly that is facing; a nanodegree less
        // isn't.
        let normal = [0.48, 0.6, 0.64];
        let facing = |ndeg: i64| Facing {
            direction: [1.0, 2.0, 2.0],
            within: Angle::from_ndeg(ndeg),
        };
        assert!(faces(normal, &facing(9_366_788_884)));
        assert!(!faces(normal, &facing(9_366_788_883)));
        // The direction scaled is the same direction.
        let scaled = Facing {
            direction: [1e-200, 2e-200, 2e-200],
            within: Angle::from_ndeg(9_366_788_884),
        };
        assert!(faces(normal, &scaled));
    }

    fn kept(index: u32) -> ElementFate {
        ElementFate {
            kept: Some(index),
            ..ElementFate::default()
        }
    }

    fn history(inputs: Vec<InputHistory>, faces: u32, edges: u32, topology: Topology) -> History {
        let history = History {
            inputs,
            result: ElementCounts {
                faces,
                edges,
                vertices: 0,
                solids: 1,
            },
            ends: Default::default(),
            topology,
        };
        let count = history.inputs.len();
        history.validate(count).unwrap();
        history
    }

    #[test]
    fn stable_keeps_an_element_whose_own_name_is_unstable_and_an_alias_stable() {
        // No layout of real solids gives an element an alias, so a map made by hand does
        // (the design's rule: `stable: true` keeps an element any of whose names is
        // stable). First a cut whose Face1 nothing reports, its outer wire's Edge2 named by
        // nothing either: named by its number, `aaaaaaaa.cut.Face1`, unstable (N3). Then a
        // fusion that keeps that face and another solid's Face1 as its one face: its names
        // are the cut's, carried unstable, then the other's, `bbbbbbbb.fuse.Face1`, stable.
        let block = ElementMap::by_number(
            id("kkkkkkkk"),
            OpCode::Fuse,
            ElementCounts {
                faces: 6,
                edges: 12,
                vertices: 8,
                solids: 1,
            },
        );
        let cut = history(
            vec![InputHistory {
                faces: vec![],
                edges: vec![kept(1)],
                vertices: vec![],
            }],
            1,
            2,
            Topology {
                face_edges: vec![vec![1, 2]],
                outer_edges: vec![vec![1, 2]],
                edge_vertices: vec![vec![]; 2],
            },
        );
        let cut = ElementMap::operation(id("aaaaaaaa"), OpCode::Cut, &[&block], &cut);
        let other = ElementMap::by_number(
            id("bbbbbbbb"),
            OpCode::Fuse,
            ElementCounts {
                faces: 1,
                edges: 0,
                vertices: 0,
                solids: 1,
            },
        );
        let fused = history(
            vec![
                InputHistory {
                    faces: vec![kept(1)],
                    edges: vec![ElementFate::default(); 2],
                    vertices: vec![],
                },
                InputHistory {
                    faces: vec![kept(1)],
                    edges: vec![],
                    vertices: vec![],
                },
            ],
            1,
            0,
            Topology {
                face_edges: vec![vec![]],
                outer_edges: vec![vec![]],
                edge_vertices: vec![],
            },
        );
        let fusion = id("rrrrrrrr");
        let map = ElementMap::operation(fusion.clone(), OpCode::Fuse, &[&cut, &other], &fused);
        let description = Description {
            faces: vec![FaceDescription {
                surface: Surface::Other,
                area: 1.0,
                centroid: [0.0; 3],
            }],
            edges: vec![],
            vertices: vec![],
        };
        let built = crate::elements::entries(&map, &fusion, &description).unwrap();
        let [face] = built.as_slice() else {
            panic!("{built:?}")
        };
        assert_eq!(face.name.to_string(), "aaaaaaaa.cut.Face1");
        assert!(!face.stable);
        let aliases: Vec<(String, bool)> = face
            .aliases
            .iter()
            .map(|a| (a.name.to_string(), a.stable))
            .collect();
        assert_eq!(aliases, [("bbbbbbbb.fuse.Face1".to_owned(), true)]);
        let mut q = Query::of(ElementKind::Face, 10);
        q.stable = Some(true);
        assert!(matches(face, &q), "kept for its stable alias");
        q.stable = Some(false);
        assert!(!matches(face, &q), "it has a stable name");
        // Its alias unstable too: no stable name, so `false` keeps it and `true` doesn't.
        let mut unstable = face.clone();
        unstable.aliases[0].stable = false;
        assert!(matches(&unstable, &q));
        q.stable = Some(true);
        assert!(!matches(&unstable, &q));
        // Its own name stable and no alias: `true` keeps it.
        let mut own = face.clone();
        own.stable = true;
        own.aliases.clear();
        assert!(matches(&own, &q));
    }
}
