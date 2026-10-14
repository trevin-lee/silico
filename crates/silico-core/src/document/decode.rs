//! A document from the reader's tree: every key known, every value of its type, every
//! number exact. A refusal is an `invalid_input` error naming the feature (its ID, or its
//! place in `features` when the ID itself is at fault), the field (`parameter`, dotted:
//! `position.2`), and where in the text.

use std::fmt;

use super::json::{At, Member, Node, SyntaxError, Value};
use super::model::{
    Document, FEATURE_TYPES, Feature, FeatureId, FormatVersion, ID_LENGTH, Operation, Position,
    Units,
};
use super::quantity::{DecimalError, LENGTH_DECIMALS, Length};
use crate::{Error, ErrorCode};

/// The document `bytes` hold, or why they don't hold one.
pub fn decode(bytes: &[u8]) -> Result<Document, Error> {
    let root = super::json::read(bytes).map_err(syntax)?;
    let mut top = Fields::of(root, None, "the document")?;
    let silico = top.take("silico")?;
    let version = integer(&silico, None, "silico")?;
    if version != u64::from(FormatVersion::NUMBER) {
        return Err(refuse(
            None,
            "silico",
            silico.at,
            format!("the document is format version {version}, which this Silico doesn't read"),
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
    fn of(node: Node, feature: Option<&str>, what: &str) -> Result<Self, Error> {
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

    /// Refuses any member not taken: the first, as written.
    fn finish(self) -> Result<(), Error> {
        match self.members.first() {
            None => Ok(()),
            Some(extra) => Err(refuse(
                self.feature(),
                &self.name(&extra.key),
                extra.at,
                format!("{} has a key it can't have: {}", self.what, extra.key),
                format!("remove {} (or check its spelling)", extra.key),
            )),
        }
    }
}

/// A non-negative whole number, as `silico` and `v` are written.
fn integer(node: &Node, feature: Option<&str>, parameter: &str) -> Result<u64, Error> {
    let Value::Number(text) = &node.value else {
        return Err(wrong_kind(feature, parameter, node, "a whole number"));
    };
    text.parse::<u64>().map_err(|_| {
        refuse(
            feature,
            parameter,
            node.at,
            format!("{parameter} is {text}, not a whole number"),
            format!("write {parameter} as a whole number, such as 1"),
        )
    })
}

fn string(node: Node, feature: Option<&str>, parameter: &str) -> Result<String, Error> {
    match node.value {
        Value::String(s) => Ok(s),
        _ => Err(wrong_kind(feature, parameter, &node, "a string")),
    }
}

fn units(node: Node) -> Result<(), Error> {
    let mut fields = Fields::of(node, None, "units")?.under("units");
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
                format!("the document's {key} is {got:?}, which format version 1 doesn't read"),
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
            format!("{parameter} {text:?} isn't a feature ID"),
            format!(
                "an ID is {ID_LENGTH} characters of digits and lower-case letters but i, l, o, and u"
            ),
        )
    })
}

fn feature(index: usize, node: Node) -> Result<Feature, Error> {
    let place = format!("features[{index}]");
    let mut fields = Fields::of(node, Some(&place), &place)?;
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
    let version = integer(&version_node, feature, "v")?;
    if !FEATURE_TYPES.contains(&type_name.as_str()) {
        return Err(refuse(
            feature,
            "type",
            type_at,
            format!("{type_name:?} isn't a feature type"),
            format!("the types are {}", FEATURE_TYPES.join(", ")),
        ));
    }
    if version != 1 {
        return Err(refuse(
            feature,
            "v",
            version_node.at,
            format!("this Silico has no {type_name} of version {version}"),
            format!("this Silico reads {type_name} version 1"),
        ));
    }
    let name_node = fields.take("name")?;
    let name_at = name_node.at;
    let name = string(name_node, feature, "name")?;
    if name.is_empty() {
        return Err(refuse(
            feature,
            "name",
            name_at,
            "name is empty",
            "give the feature a name",
        ));
    }
    let operation = operation(&type_name, &mut fields)?;
    let note = match fields.optional("note") {
        Some(node) => Some(string(node, fields.feature(), "note")?),
        None => None,
    };
    fields.finish()?;
    Ok(Feature {
        id,
        operation,
        version: 1,
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
            format!("{parameter} {text} {e}"),
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
