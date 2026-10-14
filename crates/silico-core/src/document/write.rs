//! A document in canonical form (docs/design/m2.1-document-format.md, D7): keys in schema
//! order; an object or array on one line when it holds no object, otherwise one member per
//! line, indented two spaces; `, ` between members on a line and `: ` after keys; strings
//! as written, escaping only `"`, `\`, and control characters; LF, and a trailing newline.
//! Nothing comes from the bytes a document was read from, so every save writes the same
//! bytes for the same document.

use super::model::{Document, Feature, FeatureId, FormatVersion, Operation, Position, Units};
use super::quantity::Length;

/// The document's canonical text.
pub fn write(document: &Document) -> String {
    let mut out = String::new();
    put(&mut out, &tree(document), 0);
    out.push('\n');
    out
}

/// A value to write: a number's or literal's text, a string, an array, or an object.
enum Out {
    Raw(String),
    Str(String),
    Array(Vec<Out>),
    Object(Vec<(&'static str, Out)>),
}

impl Out {
    /// Whether it is, or holds, an object anywhere inside it.
    fn holds_object(&self) -> bool {
        match self {
            Out::Raw(_) | Out::Str(_) => false,
            Out::Object(_) => true,
            Out::Array(items) => items.iter().any(Out::holds_object),
        }
    }

    /// Whether it's written on one line: it holds no object (an object of scalars and
    /// arrays of them counts as holding none).
    fn one_line(&self) -> bool {
        match self {
            Out::Raw(_) | Out::Str(_) => true,
            Out::Array(items) => !items.iter().any(Out::holds_object),
            Out::Object(members) => !members.iter().any(|(_, v)| v.holds_object()),
        }
    }
}

fn tree(document: &Document) -> Out {
    let Document {
        silico: FormatVersion,
        units: Units,
        features,
    } = document;
    Out::Object(vec![
        ("silico", Out::Raw(FormatVersion::NUMBER.to_string())),
        (
            "units",
            Out::Object(vec![
                ("length", Out::Str(Units::LENGTH.to_owned())),
                ("resolution", Out::Str(Units::RESOLUTION.to_owned())),
            ]),
        ),
        (
            "features",
            Out::Array(features.iter().map(feature).collect()),
        ),
    ])
}

fn feature(feature: &Feature) -> Out {
    let Feature {
        id,
        operation,
        version,
        name,
        note,
    } = feature;
    let mut members = vec![
        ("id", reference(id)),
        ("type", Out::Str(operation.type_name().to_owned())),
        ("v", Out::Raw(version.to_string())),
        ("name", Out::Str(name.clone())),
    ];
    members.extend(fields(operation));
    if let Some(note) = note {
        members.push(("note", Out::Str(note.clone())));
    }
    Out::Object(members)
}

/// The operation's own fields, in schema order.
fn fields(operation: &Operation) -> Vec<(&'static str, Out)> {
    match operation {
        Operation::Box {
            position,
            length,
            width,
            height,
        } => vec![
            ("position", point(position)),
            ("length", number(*length)),
            ("width", number(*width)),
            ("height", number(*height)),
        ],
        Operation::Cylinder {
            position,
            radius,
            height,
        } => vec![
            ("position", point(position)),
            ("radius", number(*radius)),
            ("height", number(*height)),
        ],
        Operation::Sphere { position, radius } => {
            vec![("position", point(position)), ("radius", number(*radius))]
        }
        Operation::Cone {
            position,
            radius1,
            radius2,
            height,
        } => vec![
            ("position", point(position)),
            ("radius1", number(*radius1)),
            ("radius2", number(*radius2)),
            ("height", number(*height)),
        ],
        Operation::Torus {
            position,
            radius1,
            radius2,
        } => vec![
            ("position", point(position)),
            ("radius1", number(*radius1)),
            ("radius2", number(*radius2)),
        ],
        Operation::Fuse { base, tool }
        | Operation::Cut { base, tool }
        | Operation::Common { base, tool } => {
            vec![("base", reference(base)), ("tool", reference(tool))]
        }
    }
}

fn number(length: Length) -> Out {
    Out::Raw(length.to_string())
}

fn point(position: &Position) -> Out {
    Out::Array(position.iter().map(|&l| number(l)).collect())
}

fn reference(id: &FeatureId) -> Out {
    Out::Str(id.as_str().to_owned())
}

fn put(out: &mut String, value: &Out, indent: usize) {
    match value {
        Out::Raw(text) => out.push_str(text),
        Out::Str(text) => quote(out, text),
        Out::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
            } else if value.one_line() {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    put(out, item, indent);
                }
                out.push(']');
            } else {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(out, indent + 1);
                    put(out, item, indent + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                pad(out, indent);
                out.push(']');
            }
        }
        Out::Object(members) => {
            if members.is_empty() {
                out.push_str("{}");
            } else if value.one_line() {
                out.push('{');
                for (i, (key, member)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    quote(out, key);
                    out.push_str(": ");
                    put(out, member, indent);
                }
                out.push('}');
            } else {
                out.push_str("{\n");
                for (i, (key, member)) in members.iter().enumerate() {
                    pad(out, indent + 1);
                    quote(out, key);
                    out.push_str(": ");
                    put(out, member, indent + 1);
                    out.push_str(if i + 1 < members.len() { ",\n" } else { "\n" });
                }
                pad(out, indent);
                out.push('}');
            }
        }
    }
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// `text` as a JSON string, escaping only what JSON requires: `"`, `\`, and the control
/// characters (the short escapes where JSON has them, `\u00xx` otherwise).
fn quote(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
