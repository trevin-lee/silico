//! A document from the reader's tree: every key known, every value of its type, every
//! number exact. A refusal is an `invalid_input` error naming the feature (its ID, or its
//! place in `features` when the ID itself is at fault), the field (`parameter`, dotted:
//! `position.2`), and where in the text.

use std::fmt;

use super::json::{At, Member, Node, SyntaxError, Value, shown};
use super::model::{
    Document, FEATURE_TYPES, Feature, FeatureId, FeatureVersion, FormatVersion, ID_LENGTH, Name,
    Operation, Position, Units,
};
use super::quantity::{DecimalError, LENGTH_DECIMALS, Length};
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
            "this Silico reads format version 1: open it in the Silico that wrote it",
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
/// digits, and `_`, at most 40), quoted as messages show it otherwise.
fn field_name(key: &str) -> String {
    let plain = !key.is_empty()
        && key.len() <= 40
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if plain { key.to_owned() } else { shown(key) }
}

/// A non-negative whole number's digits, as `silico` and `v` are written: compared as text,
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

/// The most of a number's text an error repeats: enough to recognize it, not a megabyte.
const SHOWN_DIGITS: usize = 40;

/// A number's text as an error shows it, cut short if it's long. (The reader has checked
/// its characters: digits, `-`, `.`, `e`, `+`.)
fn shown_number(text: &str) -> String {
    match text.get(..SHOWN_DIGITS) {
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
                    "the document's {key} is {}, which format version 1 doesn't read",
                    shown(&got)
                ),
                "version 1 reads only {\"length\": \"mm\", \"resolution\": \"1nm\"}",
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
            format!("this Silico reads {type_name} version 1"),
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
    fields.finish()?;
    Ok(Feature {
        id,
        operation,
        version: FeatureVersion,
        name,
        note,
    })
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
        // FEATURE_TYPES was checked: the last is `common`.
        _ => Operation::Common {
            base: reference(fields, "base")?,
            tool: reference(fields, "tool")?,
        },
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
        return Err(wrong_kind(
            feature,
            parameter,
            node,
            "a length in millimetres",
        ));
    };
    Length::parse(text).map_err(|e| {
        let suggestion = match e {
            DecimalError::Exponent => {
                "write it as a plain decimal, such as 0.001 for 1e-3".to_owned()
            }
            DecimalError::TooPrecise { decimals } => {
                format!("round it to {decimals} decimals: lengths are whole nanometres")
            }
            DecimalError::OutOfRange => format!(
                "a length is at most {} mm either way",
                Length::from_nm(i64::MAX)
            ),
            DecimalError::NegativeZero => "write 0".to_owned(),
            DecimalError::Malformed => {
                format!("write a plain decimal, to at most {LENGTH_DECIMALS} decimals")
            }
        };
        refuse(
            feature,
            parameter,
            node.at,
            format!("{parameter} {} {e}", shown_number(text)),
            suggestion,
        )
    })
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
