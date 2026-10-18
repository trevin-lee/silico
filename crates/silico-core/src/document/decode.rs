//! A document from the reader's tree: every key known, every value of its type, every
//! number exact. A refusal is an `invalid_input` error naming the feature (its ID, or its
//! place in `features` when the ID itself is at fault), the field (`parameter`, dotted:
//! `position.2`), and where in the text.

use std::fmt;

use super::fingerprint::{Fingerprint, MEASURE_DECIMALS, Measure};
use super::json::{At, Member, Node, SHOWN_CHARS, SyntaxError, Value, shown};
use super::model::{
    Document, FEATURE_TYPES, Feature, FeatureId, FeatureVersion, FormatVersion, ID_LENGTH, Name,
    Operation, Position, Units,
};
use super::quantity::{ANGLE_DECIMALS, Angle, LENGTH_DECIMALS, Length, NumberError};
use super::sketch::{
    CONSTRAINT_TYPES, Coordinate, Direction, ElementId, Form, LineSide, Plane, Reference, Sketch,
    SketchArc, SketchCircle, SketchConstraint, SketchLine, SketchPoint, TangentSide,
};
use crate::{Error, ErrorCode};

/// The document `bytes` hold, or why they don't hold one.
pub fn decode(bytes: &[u8]) -> Result<Document, Error> {
    let root = super::json::read(bytes).map_err(syntax)?;
    let mut top = Fields::of(root, None, None, "the document")?;
    let silico = top.take("silico")?;
    let version = whole_number(&silico, None, "silico")?;
    if version != FormatVersion::NUMBER.to_string() {
        return Err(refuse(
            None,
            "silico",
            silico.at,
            format!(
                "the document is format version {}, which this Silico doesn't read",
                shown_number(&version)
            ),
            format!(
                "this Silico reads format version {}: open it in the Silico that wrote it",
                FormatVersion::NUMBER
            ),
        ));
    }
    units(top.take("units")?)?;
    let features = top.take("features")?;
    let Value::Array(items) = features.value else {
        return Err(wrong_kind(
            None,
            "features",
            &features,
            "an array of features",
        ));
    };
    top.finish()?;
    let features = items
        .into_iter()
        .enumerate()
        .map(|(index, node)| feature(index, node))
        .collect::<Result<_, _>>()?;
    Ok(Document {
        silico: FormatVersion,
        units: Units,
        features,
    })
}

/// The reader's refusal, as an error.
fn syntax(e: SyntaxError) -> Error {
    if e.cause == super::json::Cause::Size {
        return Error::new(
            ErrorCode::InvalidInput,
            format!("the document is too large for Silico to read: {}", e.message),
        )
        .with_suggestion(format!(
            "make it smaller: Silico reads at most {} bytes, and a document's values take at most about {} bytes to hold",
            super::json::MAX_INPUT_LEN,
            super::json::MAX_TREE_BYTES
        ));
    }
    Error::new(
        ErrorCode::InvalidInput,
        format!("the document isn't valid JSON: {}, at {}", e.message, e.at),
    )
    .with_suggestion("fix the text there: a document is strict JSON (RFC 8259)")
}

/// An `invalid_input` error about `parameter` of `feature` (none for the document's own
/// fields), at `at`.
fn refuse(
    feature: Option<&str>,
    parameter: &str,
    at: At,
    message: impl fmt::Display,
    suggestion: impl Into<String>,
) -> Error {
    let error = Error::new(ErrorCode::InvalidInput, format!("{message}, at {at}"))
        .with_parameter(parameter)
        .with_suggestion(suggestion);
    match feature {
        Some(feature) => error.with_feature(feature),
        None => error,
    }
}

/// What `parameter` was, and what it should have been.
fn wrong_kind(feature: Option<&str>, parameter: &str, node: &Node, want: &str) -> Error {
    refuse(
        feature,
        parameter,
        node.at,
        format!("{parameter} is {}, not {want}", node.value.kind()),
        format!("write {parameter} as {want}"),
    )
}

/// An object's members, taken one by one; what's left is unknown.
struct Fields {
    members: Vec<Member>,
    feature: Option<String>,
    /// Where the object starts, for a missing key.
    at: At,
    /// What the object is, for messages (`the document`, `units`, `feature k7f2qx3m`).
    what: String,
    /// The dotted name of the object's field, before its members' (`units.`).
    prefix: String,
}

impl Fields {
    /// The members of `node`, an object: `what` it is for messages, and its field
    /// (`parameter`), if it's one.
    fn of(
        node: Node,
        feature: Option<&str>,
        parameter: Option<&str>,
        what: &str,
    ) -> Result<Self, Error> {
        let Value::Object(members) = node.value else {
            let error = Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "{what} is {}, not an object, at {}",
                    node.value.kind(),
                    node.at
                ),
            )
            .with_suggestion(format!("write {what} as an object"));
            let error = match parameter {
                Some(parameter) => error.with_parameter(parameter),
                None => error,
            };
            return Err(match feature {
                Some(feature) => error.with_feature(feature),
                None => error,
            });
        };
        Ok(Fields {
            members,
            feature: feature.map(str::to_owned),
            at: node.at,
            what: what.to_owned(),
            prefix: String::new(),
        })
    }

    fn under(mut self, prefix: &str) -> Self {
        self.prefix = format!("{prefix}.");
        self
    }

    fn feature(&self) -> Option<&str> {
        self.feature.as_deref()
    }

    fn name(&self, key: &str) -> String {
        format!("{}{key}", self.prefix)
    }

    /// The member `key`, which must be there.
    fn take(&mut self, key: &str) -> Result<Node, Error> {
        self.optional(key).ok_or_else(|| {
            refuse(
                self.feature(),
                &self.name(key),
                self.at,
                format!("{} has no {key}", self.what),
                format!("add {key} to {}", self.what),
            )
        })
    }

    /// The member `key`, if it's there.
    fn optional(&mut self, key: &str) -> Option<Node> {
        let index = self.members.iter().position(|m| m.key == key)?;
        Some(self.members.remove(index).value)
    }

    /// Refuses any member not taken: the first, as written. The key is quoted, and cut
    /// short if it's long, wherever the error shows it.
    fn finish(self) -> Result<(), Error> {
        match self.members.first() {
            None => Ok(()),
            Some(extra) => {
                let key = shown(&extra.key);
                Err(refuse(
                    self.feature(),
                    &self.name(&field_name(&extra.key)),
                    extra.at,
                    format!("{} has a key it can't have: {key}", self.what),
                    format!("remove {key} (or check its spelling)"),
                ))
            }
        }
    }
}

/// An unknown key as `parameter` names it: as written if it's a plain name (letters,
/// digits, and `_`, at most [`SHOWN_CHARS`]), quoted as messages show it otherwise.
fn field_name(key: &str) -> String {
    let plain = !key.is_empty()
        && key.len() <= SHOWN_CHARS
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if plain { key.to_owned() } else { shown(key) }
}

/// A non-negative whole number's digits, as `silico`, `v`, and a fingerprint's counts are
/// written: compared as text,
/// so no number is too large to say which it is.
fn whole_number(node: &Node, feature: Option<&str>, parameter: &str) -> Result<String, Error> {
    let Value::Number(text) = &node.value else {
        return Err(wrong_kind(feature, parameter, node, "a whole number"));
    };
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(text.clone());
    }
    Err(refuse(
        feature,
        parameter,
        node.at,
        format!("{parameter} is {}, not a whole number", shown_number(text)),
        format!("write {parameter} as a whole number, such as 1"),
    ))
}

/// A number's text as an error shows it, cut short if it's long. (The reader has checked
/// its characters: digits, `-`, `.`, `e`, `+`.)
fn shown_number(text: &str) -> String {
    match text.get(..SHOWN_CHARS) {
        Some(start) if start.len() < text.len() => format!("{start}…"),
        _ => text.to_owned(),
    }
}

fn string(node: Node, feature: Option<&str>, parameter: &str) -> Result<String, Error> {
    match node.value {
        Value::String(s) => Ok(s),
        _ => Err(wrong_kind(feature, parameter, &node, "a string")),
    }
}

fn units(node: Node) -> Result<(), Error> {
    let mut fields = Fields::of(node, None, Some("units"), "units")?.under("units");
    for (key, want) in [("length", Units::LENGTH), ("resolution", Units::RESOLUTION)] {
        let value = fields.take(key)?;
        let at = value.at;
        let name = format!("units.{key}");
        let got = string(value, None, &name)?;
        if got != want {
            return Err(refuse(
                None,
                &name,
                at,
                format!(
                    "the document's {key} is {}, which format version {} doesn't read",
                    shown(&got),
                    FormatVersion::NUMBER
                ),
                format!(
                    "version {} reads only {{\"length\": \"{}\", \"resolution\": \"{}\"}}",
                    FormatVersion::NUMBER,
                    Units::LENGTH,
                    Units::RESOLUTION
                ),
            ));
        }
    }
    fields.finish()
}

fn id(node: Node, feature: Option<&str>, parameter: &str) -> Result<FeatureId, Error> {
    let at = node.at;
    let text = string(node, feature, parameter)?;
    FeatureId::new(&text).ok_or_else(|| {
        refuse(
            feature,
            parameter,
            at,
            format!("{parameter} {} isn't a feature ID", shown(&text)),
            format!(
                "an ID is {ID_LENGTH} characters of digits and lower-case letters but i, l, o, and u"
            ),
        )
    })
}

fn feature(index: usize, node: Node) -> Result<Feature, Error> {
    let place = format!("features[{index}]");
    let mut fields = Fields::of(node, Some(&place), None, &place)?;
    let id = id(fields.take("id")?, Some(&place), "id")?;
    // From here on, the feature is named by its ID.
    let label = id.to_string();
    fields.feature = Some(label.clone());
    fields.what = format!("feature {label}");
    let feature = Some(label.as_str());
    let type_node = fields.take("type")?;
    let type_at = type_node.at;
    let type_name = string(type_node, feature, "type")?;
    let version_node = fields.take("v")?;
    let version = whole_number(&version_node, feature, "v")?;
    if !FEATURE_TYPES.contains(&type_name.as_str()) {
        return Err(refuse(
            feature,
            "type",
            type_at,
            format!("{} isn't a feature type", shown(&type_name)),
            format!("the types are {}", FEATURE_TYPES.join(", ")),
        ));
    }
    if version != FeatureVersion::NUMBER.to_string() {
        return Err(refuse(
            feature,
            "v",
            version_node.at,
            format!(
                "this Silico has no {type_name} of version {}",
                shown_number(&version)
            ),
            format!(
                "this Silico reads {type_name} version {}",
                FeatureVersion::NUMBER
            ),
        ));
    }
    let name_node = fields.take("name")?;
    let name_at = name_node.at;
    let name = Name::new(string(name_node, feature, "name")?).ok_or_else(|| {
        refuse(
            feature,
            "name",
            name_at,
            "name is empty",
            "give the feature a name",
        )
    })?;
    let operation = operation(&type_name, &mut fields)?;
    let note = match fields.optional("note") {
        Some(node) => Some(string(node, fields.feature(), "note")?),
        None => None,
    };
    let fingerprint = match fields.optional("fingerprint") {
        Some(node) => Some(fingerprint(node, fields.feature())?),
        None => None,
    };
    fields.finish()?;
    Ok(Feature {
        id,
        operation,
        version: FeatureVersion,
        name,
        note,
        fingerprint,
    })
}

fn fingerprint(node: Node, feature: Option<&str>) -> Result<Fingerprint, Error> {
    let mut fields =
        Fields::of(node, feature, Some("fingerprint"), "fingerprint")?.under("fingerprint");
    let volume = measure(&mut fields, "volume")?;
    let area = measure(&mut fields, "area")?;
    let bbox = match fields.optional("bbox") {
        Some(node) => Some(bbox(&node, fields.feature())?),
        None => None,
    };
    let faces = count(&mut fields, "faces")?;
    let edges = count(&mut fields, "edges")?;
    let vertices = count(&mut fields, "vertices")?;
    fields.finish()?;
    Ok(Fingerprint {
        volume,
        area,
        bbox,
        faces,
        edges,
        vertices,
    })
}

fn measure(fields: &mut Fields, key: &str) -> Result<Measure, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let feature = fields.feature();
    let Value::Number(text) = &node.value else {
        return Err(wrong_kind(feature, &name, &node, "a number"));
    };
    Measure::parse(text).map_err(|e| {
        number_refused(
            feature,
            &name,
            &node,
            text,
            &e,
            Refused {
                precise: String::new(),
                range: format!(
                    "a fingerprint's {key} is at most {} either way",
                    Measure::from_millionths(i128::MAX)
                ),
                decimals: MEASURE_DECIMALS,
            },
        )
    })
}

fn count(fields: &mut Fields, key: &str) -> Result<u32, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let feature = fields.feature();
    let digits = whole_number(&node, feature, &name)?;
    digits.parse::<u32>().map_err(|_| {
        refuse(
            feature,
            &name,
            node.at,
            format!("{name} {} is too large", shown_number(&digits)),
            format!("a fingerprint's {key} is at most {}", u32::MAX),
        )
    })
}

fn bbox(node: &Node, feature: Option<&str>) -> Result<[Length; 6], Error> {
    let want = "an array of six lengths, [x, y, z, x, y, z]";
    let Value::Array(items) = &node.value else {
        return Err(wrong_kind(feature, "fingerprint.bbox", node, want));
    };
    let [a, b, c, d, e, f] = items.as_slice() else {
        return Err(refuse(
            feature,
            "fingerprint.bbox",
            node.at,
            format!("fingerprint.bbox has {} values, not 6", items.len()),
            format!("write fingerprint.bbox as {want}"),
        ));
    };
    let mut corners = [Length::default(); 6];
    for (i, (corner, item)) in corners.iter_mut().zip([a, b, c, d, e, f]).enumerate() {
        *corner = length_value(item, feature, &format!("fingerprint.bbox.{i}"))?;
    }
    Ok(corners)
}

fn operation(type_name: &str, fields: &mut Fields) -> Result<Operation, Error> {
    Ok(match type_name {
        "box" => Operation::Box {
            position: position(fields)?,
            length: length(fields, "length")?,
            width: length(fields, "width")?,
            height: length(fields, "height")?,
        },
        "cylinder" => Operation::Cylinder {
            position: position(fields)?,
            radius: length(fields, "radius")?,
            height: length(fields, "height")?,
        },
        "sphere" => Operation::Sphere {
            position: position(fields)?,
            radius: length(fields, "radius")?,
        },
        "cone" => Operation::Cone {
            position: position(fields)?,
            radius1: length(fields, "radius1")?,
            radius2: length(fields, "radius2")?,
            height: length(fields, "height")?,
        },
        "torus" => Operation::Torus {
            position: position(fields)?,
            radius1: length(fields, "radius1")?,
            radius2: length(fields, "radius2")?,
        },
        "fuse" => Operation::Fuse {
            base: reference(fields, "base")?,
            tool: reference(fields, "tool")?,
        },
        "cut" => Operation::Cut {
            base: reference(fields, "base")?,
            tool: reference(fields, "tool")?,
        },
        "common" => Operation::Common {
            base: reference(fields, "base")?,
            tool: reference(fields, "tool")?,
        },
        "sketch" => Operation::Sketch(sketch(fields)?),
        // A type in FEATURE_TYPES with no arm here: Silico's bug, not the document's.
        other => {
            return Err(Error::new(
                ErrorCode::Internal,
                format!("the feature type {} has no reader", shown(other)),
            )
            .with_suggestion(crate::worker::SILICO_BUG));
        }
    })
}

fn reference(fields: &mut Fields, key: &str) -> Result<FeatureId, Error> {
    let node = fields.take(key)?;
    id(node, fields.feature(), key)
}

fn length(fields: &mut Fields, key: &str) -> Result<Length, Error> {
    let node = fields.take(key)?;
    length_value(&node, fields.feature(), key)
}

fn length_value(node: &Node, feature: Option<&str>, parameter: &str) -> Result<Length, Error> {
    let Value::Number(text) = &node.value else {
        // Strings are reserved for expressions (`"=width * 2"`), which arrive later.
        if let Value::String(_) = &node.value {
            return Err(refuse(
                feature,
                parameter,
                node.at,
                format!("{parameter} is a string, not a length in millimetres"),
                format!(
                    "write {parameter} as a number, such as 10.5: strings are reserved for expressions, which this Silico doesn't read yet"
                ),
            ));
        }
        return Err(wrong_kind(
            feature,
            parameter,
            node,
            "a length in millimetres",
        ));
    };
    Length::parse(text).map_err(|e| {
        number_refused(
            feature,
            parameter,
            node,
            text,
            &e,
            Refused {
                precise: ": lengths are whole nanometres".to_owned(),
                range: format!(
                    "a length is at most {} mm either way",
                    Length::from_nm(i64::MAX)
                ),
                decimals: LENGTH_DECIMALS,
            },
        )
    })
}

/// What to say of a number a quantity refused: after "round it to N decimals" (`precise`),
/// how large it may be (`range`), and its decimals.
struct Refused {
    precise: String,
    range: String,
    decimals: u32,
}

/// The refusal of `text`, the number at `node` in `parameter`, for `e`.
fn number_refused(
    feature: Option<&str>,
    parameter: &str,
    node: &Node,
    text: &str,
    e: &NumberError,
    said: Refused,
) -> Error {
    let suggestion = match e {
        NumberError::TooPrecise { decimals } => {
            format!("round it to {decimals} decimals{}", said.precise)
        }
        NumberError::OutOfRange => said.range,
        NumberError::NegativeZero => "write 0".to_owned(),
        NumberError::Malformed => {
            format!("write a number, to at most {} decimals", said.decimals)
        }
    };
    refuse(
        feature,
        parameter,
        node.at,
        format!("{parameter} {} {e}", shown_number(text)),
        suggestion,
    )
}

fn position(fields: &mut Fields) -> Result<Position, Error> {
    let node = fields.take("position")?;
    let feature = fields.feature();
    let want = "an array of three lengths, [x, y, z]";
    let Value::Array(items) = &node.value else {
        return Err(wrong_kind(feature, "position", &node, want));
    };
    let [x, y, z] = items.as_slice() else {
        return Err(refuse(
            feature,
            "position",
            node.at,
            format!("position has {} values, not 3", items.len()),
            format!("write position as {want}"),
        ));
    };
    Ok([
        length_value(x, feature, "position.0")?,
        length_value(y, feature, "position.1")?,
        length_value(z, feature, "position.2")?,
    ])
}

/// A sketch's fields (docs/design/m3.2-sketch-format.md). Each list is kept in the order
/// written; the writer sorts it by ID.
fn sketch(fields: &mut Fields) -> Result<Sketch, Error> {
    let plane = plane(fields)?;
    let points = list(fields, "points", point)?;
    let lines = list(fields, "lines", line)?;
    let circles = list(fields, "circles", circle)?;
    let arcs = list(fields, "arcs", arc)?;
    let constraints = list(fields, "constraints", constraint)?;
    Ok(Sketch {
        plane,
        points,
        lines,
        circles,
        arcs,
        constraints,
    })
}

fn plane(fields: &mut Fields) -> Result<Plane, Error> {
    let node = fields.take("plane")?;
    let at = node.at;
    let text = string(node, fields.feature(), "plane")?;
    Plane::ALL
        .into_iter()
        .find(|p| p.as_str() == text)
        .ok_or_else(|| {
            refuse(
                fields.feature(),
                "plane",
                at,
                format!("plane {} isn't a sketch plane", shown(&text)),
                "the planes are xy, xz, and yz",
            )
        })
}

/// The list `key`, each item an object read by `item` as `key.i`.
fn list<T>(
    fields: &mut Fields,
    key: &str,
    item: fn(&mut Fields) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let node = fields.take(key)?;
    let feature = fields.feature().map(str::to_owned);
    let feature = feature.as_deref();
    let Value::Array(items) = node.value else {
        return Err(wrong_kind(
            feature,
            key,
            &node,
            &format!("an array of {key}"),
        ));
    };
    items
        .into_iter()
        .enumerate()
        .map(|(i, node)| {
            let name = format!("{key}.{i}");
            let mut fields = Fields::of(node, feature, Some(&name), &name)?.under(&name);
            let value = item(&mut fields)?;
            fields.finish()?;
            Ok(value)
        })
        .collect()
}

fn point(fields: &mut Fields) -> Result<SketchPoint, Error> {
    Ok(SketchPoint {
        id: element_id(fields, "id")?,
        x: coordinate(fields, "x")?,
        y: coordinate(fields, "y")?,
        construction: boolean(fields, "construction")?,
    })
}

fn line(fields: &mut Fields) -> Result<SketchLine, Error> {
    Ok(SketchLine {
        id: element_id(fields, "id")?,
        from: element_id(fields, "from")?,
        to: element_id(fields, "to")?,
        construction: boolean(fields, "construction")?,
    })
}

fn circle(fields: &mut Fields) -> Result<SketchCircle, Error> {
    Ok(SketchCircle {
        id: element_id(fields, "id")?,
        center: element_id(fields, "center")?,
        radius: coordinate(fields, "radius")?,
        construction: boolean(fields, "construction")?,
    })
}

fn arc(fields: &mut Fields) -> Result<SketchArc, Error> {
    Ok(SketchArc {
        id: element_id(fields, "id")?,
        center: element_id(fields, "center")?,
        start: element_id(fields, "start")?,
        end: element_id(fields, "end")?,
        construction: boolean(fields, "construction")?,
    })
}

fn constraint(fields: &mut Fields) -> Result<SketchConstraint, Error> {
    let id = element_id(fields, "id")?;
    let type_node = fields.take("type")?;
    let type_at = type_node.at;
    let type_name = string(type_node, fields.feature(), &fields.name("type"))?;
    let r = sketch_reference;
    let form = match type_name.as_str() {
        "horizontal" => Form::Horizontal {
            line: r(fields, "line")?,
        },
        "vertical" => Form::Vertical {
            line: r(fields, "line")?,
        },
        "horizontal_points" => Form::HorizontalPoints {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
        },
        "vertical_points" => Form::VerticalPoints {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
        },
        "length" => Form::Length {
            line: r(fields, "line")?,
            value: sketch_length(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "distance" => Form::Distance {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            value: sketch_length(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "distance_x" => Form::DistanceX {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            value: sketch_length(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "distance_y" => Form::DistanceY {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            value: sketch_length(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "point_line_distance" => Form::PointLineDistance {
            point: r(fields, "point")?,
            line: r(fields, "line")?,
            value: sketch_length(fields, "value")?,
            side: word(fields, "side", &LineSide::ALL.map(|w| (w.as_str(), w)))?,
            driving: boolean(fields, "driving")?,
        },
        "on" => Form::On {
            point: r(fields, "point")?,
            curve: r(fields, "curve")?,
        },
        "parallel" => Form::Parallel {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
        },
        "perpendicular" => Form::Perpendicular {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
        },
        "angle" => Form::Angle {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            value: angle(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "radius" => Form::Radius {
            curve: r(fields, "curve")?,
            value: sketch_length(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "diameter" => Form::Diameter {
            curve: r(fields, "curve")?,
            value: sketch_length(fields, "value")?,
            driving: boolean(fields, "driving")?,
        },
        "equal" => Form::Equal {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
        },
        "tangent" => Form::Tangent {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            side: word(fields, "side", &TangentSide::ALL.map(|w| (w.as_str(), w)))?,
        },
        "tangent_on" => Form::TangentOn {
            a: r(fields, "a")?,
            at: r(fields, "at")?,
            b: r(fields, "b")?,
            direction: word(
                fields,
                "direction",
                &Direction::ALL.map(|w| (w.as_str(), w)),
            )?,
        },
        "tangent_at" => Form::TangentAt {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            at: r(fields, "at")?,
            direction: word(
                fields,
                "direction",
                &Direction::ALL.map(|w| (w.as_str(), w)),
            )?,
        },
        "symmetric" => Form::Symmetric {
            a: r(fields, "a")?,
            b: r(fields, "b")?,
            about: r(fields, "about")?,
        },
        "midpoint" => Form::Midpoint {
            point: r(fields, "point")?,
            line: r(fields, "line")?,
        },
        other => {
            return Err(refuse(
                fields.feature(),
                &fields.name("type"),
                type_at,
                format!("{} isn't a constraint type", shown(other)),
                format!("the types are {}", CONSTRAINT_TYPES.join(", ")),
            ));
        }
    };
    Ok(SketchConstraint { id, form })
}

fn element_id(fields: &mut Fields, key: &str) -> Result<ElementId, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let at = node.at;
    let text = string(node, fields.feature(), &name)?;
    ElementId::new(&text).ok_or_else(|| {
        refuse(
            fields.feature(),
            &name,
            at,
            format!("{name} {} isn't a sketch element's ID", shown(&text)),
            format!(
                "an ID is {ID_LENGTH} characters of digits and lower-case letters but i, l, o, and u"
            ),
        )
    })
}

fn sketch_reference(fields: &mut Fields, key: &str) -> Result<Reference, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let at = node.at;
    let text = string(node, fields.feature(), &name)?;
    Reference::new(&text).ok_or_else(|| {
        refuse(
            fields.feature(),
            &name,
            at,
            format!("{name} {} isn't a sketch element's ID, origin, x_axis, or y_axis", shown(&text)),
            format!(
                "name an element of this sketch by its ID ({ID_LENGTH} characters of digits and lower-case letters but i, l, o, and u), or origin, x_axis, or y_axis"
            ),
        )
    })
}

/// One of `words`, by its text.
fn word<T: Copy>(fields: &mut Fields, key: &str, words: &[(&str, T)]) -> Result<T, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let at = node.at;
    let text = string(node, fields.feature(), &name)?;
    words
        .iter()
        .find(|(w, _)| *w == text)
        .map(|&(_, value)| value)
        .ok_or_else(|| {
            let all: Vec<&str> = words.iter().map(|(w, _)| *w).collect();
            refuse(
                fields.feature(),
                &name,
                at,
                format!("{name} {} isn't one of {}", shown(&text), all.join(", ")),
                format!("write {name} as one of {}", all.join(", ")),
            )
        })
}

fn boolean(fields: &mut Fields, key: &str) -> Result<bool, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    match node.value {
        Value::Bool(b) => Ok(b),
        _ => Err(wrong_kind(fields.feature(), &name, &node, "true or false")),
    }
}

fn coordinate(fields: &mut Fields, key: &str) -> Result<Coordinate, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let feature = fields.feature();
    let Value::Number(text) = &node.value else {
        return Err(wrong_kind(feature, &name, &node, "a number in millimetres"));
    };
    Coordinate::parse(text).ok_or_else(|| {
        refuse(
            feature,
            &name,
            node.at,
            format!("{name} {} is too large", shown_number(text)),
            format!(
                "a sketch's coordinates and radii are within a kilometre ({} mm)",
                silico_protocol::format_number(silico_protocol::MAX_LENGTH)
            ),
        )
    })
}

/// A length inside a sketch, named by its dotted field (`constraints.3.value`).
fn sketch_length(fields: &mut Fields, key: &str) -> Result<Length, Error> {
    let node = fields.take(key)?;
    length_value(&node, fields.feature(), &fields.name(key))
}

fn angle(fields: &mut Fields, key: &str) -> Result<Angle, Error> {
    let node = fields.take(key)?;
    let name = fields.name(key);
    let feature = fields.feature();
    let Value::Number(text) = &node.value else {
        if let Value::String(_) = &node.value {
            return Err(refuse(
                feature,
                &name,
                node.at,
                format!("{name} is a string, not an angle in degrees"),
                format!(
                    "write {name} as a number, such as 45: strings are reserved for expressions, which this Silico doesn't read yet"
                ),
            ));
        }
        return Err(wrong_kind(feature, &name, &node, "an angle in degrees"));
    };
    Angle::parse(text).map_err(|e| {
        number_refused(
            feature,
            &name,
            &node,
            text,
            &e,
            Refused {
                precise: ": angles are whole nanodegrees".to_owned(),
                range: format!(
                    "an angle is at most {} degrees either way",
                    Angle::from_ndeg(i64::MAX)
                ),
                decimals: ANGLE_DECIMALS,
            },
        )
    })
}
