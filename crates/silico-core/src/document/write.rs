//! A document in canonical form (docs/design/m2.1-document-format.md, D7): keys in schema
//! order; an object or array on one line when it holds no object, otherwise one member per
//! line, indented two spaces; `, ` between members on a line and `: ` after keys; strings
//! as written, escaping only `"`, `\`, control characters, the line and paragraph
//! separators, and the bidirectional controls; LF, and a trailing newline.
//! Nothing comes from the bytes a document was read from, so every save writes the same
//! bytes for the same document. Writing stops as soon as the text passes a limit, and
//! borrows the document's strings: a document too large costs no more text than the limit
//! (a small tree of its values, one per feature, is built first, in proportion to it).

use std::fmt::Write as _;

use super::model::{
    Document, Feature, FeatureId, FeatureVersion, FormatVersion, Operation, Position, Units,
};
use super::quantity::Length;

/// The text passed the limit it was written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLong;

/// The document's canonical text, if it's at most `limit` bytes.
pub fn write(document: &Document, limit: usize) -> Result<String, TooLong> {
    let mut writer = Writer::new(limit, true);
    writer.put(&tree(document), 0)?;
    writer.push("\n")?;
    Ok(writer.out)
}

/// Whether the document's canonical text is at most `limit` bytes: counted as it would be
/// written, without keeping it.
pub fn fits(document: &Document, limit: usize) -> bool {
    let mut writer = Writer::new(limit, false);
    writer.put(&tree(document), 0).is_ok() && writer.push("\n").is_ok()
}

/// Whether `feature`'s canonical text is at most `limit` bytes.
pub fn feature_fits(feature: &Feature, limit: usize) -> bool {
    Writer::new(limit, false)
        .put(&self::feature(feature), 0)
        .is_ok()
}

/// A value to write: a number's or literal's text, a string, an array, or an object.
enum Out<'a> {
    Raw(String),
    Str(&'a str),
    Array(Vec<Out<'a>>),
    Object(Vec<(&'static str, Out<'a>)>),
}

impl Out<'_> {
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

fn tree(document: &Document) -> Out<'_> {
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
                ("length", Out::Str(Units::LENGTH)),
                ("resolution", Out::Str(Units::RESOLUTION)),
            ]),
        ),
        (
            "features",
            Out::Array(features.iter().map(feature).collect()),
        ),
    ])
}

fn feature(feature: &Feature) -> Out<'_> {
    let Feature {
        id,
        operation,
        version: FeatureVersion,
        name,
        note,
    } = feature;
    let mut members = vec![
        ("id", reference(id)),
        ("type", Out::Str(operation.type_name())),
        ("v", Out::Raw(FeatureVersion::NUMBER.to_string())),
        ("name", Out::Str(name.as_str())),
    ];
    members.extend(fields(operation));
    if let Some(note) = note {
        members.push(("note", Out::Str(note)));
    }
    Out::Object(members)
}

/// The operation's own fields, in schema order.
fn fields(operation: &Operation) -> Vec<(&'static str, Out<'_>)> {
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

fn number(length: Length) -> Out<'static> {
    Out::Raw(length.to_string())
}

fn point(position: &Position) -> Out<'static> {
    Out::Array(position.iter().map(|&l| number(l)).collect())
}

fn reference(id: &FeatureId) -> Out<'_> {
    Out::Str(id.as_str())
}

/// Canonical text, written (or only counted) up to a limit.
struct Writer {
    out: String,
    /// The text's bytes so far, kept or not.
    len: usize,
    limit: usize,
    /// Whether the text is kept, or only counted.
    keep: bool,
}

impl Writer {
    fn new(limit: usize, keep: bool) -> Self {
        Writer {
            out: String::new(),
            len: 0,
            limit,
            keep,
        }
    }

    fn check(&self) -> Result<(), TooLong> {
        if self.len > self.limit {
            Err(TooLong)
        } else {
            Ok(())
        }
    }

    fn push(&mut self, text: &str) -> Result<(), TooLong> {
        self.len += text.len();
        if self.keep {
            self.out.push_str(text);
        }
        self.check()
    }

    fn push_char(&mut self, c: char) {
        self.len += c.len_utf8();
        if self.keep {
            self.out.push(c);
        }
    }

    fn put(&mut self, value: &Out<'_>, indent: usize) -> Result<(), TooLong> {
        match value {
            Out::Raw(text) => self.push(text),
            Out::Str(text) => self.quote(text),
            Out::Array(items) => {
                if items.is_empty() {
                    self.push("[]")
                } else if value.one_line() {
                    self.push("[")?;
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            self.push(", ")?;
                        }
                        self.put(item, indent)?;
                    }
                    self.push("]")
                } else {
                    self.push("[\n")?;
                    for (i, item) in items.iter().enumerate() {
                        self.pad(indent + 1)?;
                        self.put(item, indent + 1)?;
                        self.push(if i + 1 < items.len() { ",\n" } else { "\n" })?;
                    }
                    self.pad(indent)?;
                    self.push("]")
                }
            }
            Out::Object(members) => {
                if members.is_empty() {
                    self.push("{}")
                } else if value.one_line() {
                    self.push("{")?;
                    for (i, (key, member)) in members.iter().enumerate() {
                        if i > 0 {
                            self.push(", ")?;
                        }
                        self.quote(key)?;
                        self.push(": ")?;
                        self.put(member, indent)?;
                    }
                    self.push("}")
                } else {
                    self.push("{\n")?;
                    for (i, (key, member)) in members.iter().enumerate() {
                        self.pad(indent + 1)?;
                        self.quote(key)?;
                        self.push(": ")?;
                        self.put(member, indent + 1)?;
                        self.push(if i + 1 < members.len() { ",\n" } else { "\n" })?;
                    }
                    self.pad(indent)?;
                    self.push("}")
                }
            }
        }
    }

    fn pad(&mut self, indent: usize) -> Result<(), TooLong> {
        for _ in 0..indent {
            self.push("  ")?;
        }
        Ok(())
    }

    /// `text` as a JSON string, escaping `"`, `\`, and the control characters (C0, DEL, and
    /// C1: the short escapes where JSON has them, `\u00xx` otherwise), the line and
    /// paragraph separators U+2028 and U+2029, so no character in a string breaks a line
    /// for any tool that reads the file by lines, and the bidirectional marks, embeddings,
    /// overrides, and isolates, so no string reorders how the rest of its line shows in a
    /// diff (Trojan Source, CVE-2021-42574).
    fn quote(&mut self, text: &str) -> Result<(), TooLong> {
        self.push_char('"');
        for c in text.chars() {
            match c {
                '"' => self.push("\\\"")?,
                '\\' => self.push("\\\\")?,
                '\n' => self.push("\\n")?,
                '\r' => self.push("\\r")?,
                '\t' => self.push("\\t")?,
                '\u{8}' => self.push("\\b")?,
                '\u{c}' => self.push("\\f")?,
                c if (c as u32) < 0x20 || escaped(c) => {
                    let mut escape = String::with_capacity(6);
                    // Writing to a String can't fail.
                    let _ = write!(escape, "\\u{:04x}", c as u32);
                    self.push(&escape)?;
                }
                c => {
                    self.push_char(c);
                    self.check()?;
                }
            }
        }
        self.push("\"")
    }
}

/// Whether `c` is written as an escape though JSON doesn't require it (see `quote`).
fn escaped(c: char) -> bool {
    matches!(c as u32, 0x7f..=0x9f | 0x2028 | 0x2029) || super::json::bidi_control(c)
}
