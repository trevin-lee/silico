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

use super::fingerprint::Fingerprint;
use super::model::{
    Document, ElementName, Feature, FeatureId, FeatureVersion, FormatVersion, Operation, Position,
    Units,
};
use super::quantity::{Angle, Length};
use super::sketch::{
    Coordinate, ElementId, ElementKind, Form, Plane, Reference, Sketch, SketchConstraint,
};

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
    /// An array written an item to a line, whatever it holds.
    Lines(Vec<Out<'a>>),
    Object(Vec<(&'static str, Out<'a>)>),
}

impl Out<'_> {
    /// Whether it is, or holds, an object anywhere inside it.
    fn holds_object(&self) -> bool {
        match self {
            Out::Raw(_) | Out::Str(_) => false,
            // Written over lines, as an object is.
            Out::Object(_) | Out::Lines(_) => true,
            Out::Array(items) => items.iter().any(Out::holds_object),
        }
    }

    /// Whether it's written on one line: it holds no object (an object of scalars and
    /// arrays of them counts as holding none).
    fn one_line(&self) -> bool {
        match self {
            Out::Raw(_) | Out::Str(_) => true,
            Out::Array(items) => !items.iter().any(Out::holds_object),
            Out::Lines(items) => items.is_empty(),
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
        fingerprint,
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
    if let Some(fingerprint) = fingerprint {
        members.push(("fingerprint", self::fingerprint(fingerprint)));
    }
    Out::Object(members)
}

/// A fingerprint's fields, in schema order: an object of scalars and one array, so on one
/// line.
fn fingerprint(fingerprint: &Fingerprint) -> Out<'static> {
    let Fingerprint {
        volume,
        area,
        bbox,
        faces,
        edges,
        vertices,
    } = fingerprint;
    let mut members = vec![
        ("volume", Out::Raw(volume.to_string())),
        ("area", Out::Raw(area.to_string())),
    ];
    if let Some(bbox) = bbox {
        members.push((
            "bbox",
            Out::Array(bbox.iter().map(|&l| number(l)).collect()),
        ));
    }
    members.extend([
        ("faces", Out::Raw(faces.to_string())),
        ("edges", Out::Raw(edges.to_string())),
        ("vertices", Out::Raw(vertices.to_string())),
    ]);
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
        Operation::Sketch(sketch) => self::sketch(sketch),
        Operation::Fillet {
            base,
            edges,
            radius,
        } => vec![
            ("base", reference(base)),
            ("edges", names(edges)),
            ("radius", number(*radius)),
        ],
        Operation::Chamfer { base, edges, size } => vec![
            ("base", reference(base)),
            ("edges", names(edges)),
            (
                "size",
                Out::Object(vec![("distance", number(size.distance))]),
            ),
        ],
    }
}

/// A sketch's fields (D5 of docs/design/m3.2-sketch-format.md): each list sorted by ID,
/// one element per line.
/// A sketch's plane, and each of its lists as the file writes it, by the kind of element
/// it holds: taken apart whole, so a field added to a sketch is a compile error here until
/// the file, and the outline, say it.
fn sketch_lists(sketch: &Sketch) -> (&Plane, [(ElementKind, Vec<Out<'_>>); 5]) {
    let Sketch {
        plane,
        points,
        lines,
        circles,
        arcs,
        constraints,
    } = sketch;
    fn sorted<T>(items: &[T], id: impl Fn(&T) -> &ElementId) -> Vec<&T> {
        let mut items: Vec<&T> = items.iter().collect();
        // Stable, so elements sharing an ID (which the gate refuses) keep their order.
        items.sort_by(|a, b| id(a).cmp(id(b)));
        items
    }
    let points = sorted(points, |p| &p.id)
        .into_iter()
        .map(|p| {
            Out::Object(vec![
                ("id", element(&p.id)),
                ("x", coordinate(p.x)),
                ("y", coordinate(p.y)),
                ("construction", boolean(p.construction)),
            ])
        })
        .collect();
    let lines = sorted(lines, |l| &l.id)
        .into_iter()
        .map(|l| {
            Out::Object(vec![
                ("id", element(&l.id)),
                ("from", element(&l.from)),
                ("to", element(&l.to)),
                ("construction", boolean(l.construction)),
            ])
        })
        .collect();
    let circles = sorted(circles, |c| &c.id)
        .into_iter()
        .map(|c| {
            Out::Object(vec![
                ("id", element(&c.id)),
                ("center", element(&c.center)),
                ("radius", coordinate(c.radius)),
                ("construction", boolean(c.construction)),
            ])
        })
        .collect();
    let arcs = sorted(arcs, |a| &a.id)
        .into_iter()
        .map(|a| {
            Out::Object(vec![
                ("id", element(&a.id)),
                ("center", element(&a.center)),
                ("start", element(&a.start)),
                ("end", element(&a.end)),
                ("construction", boolean(a.construction)),
            ])
        })
        .collect();
    let constraints = sorted(constraints, |c| &c.id)
        .into_iter()
        .map(constraint)
        .collect();
    (
        plane,
        [
            (ElementKind::Point, points),
            (ElementKind::Line, lines),
            (ElementKind::Circle, circles),
            (ElementKind::Arc, arcs),
            (ElementKind::Constraint, constraints),
        ],
    )
}

fn sketch(sketch: &Sketch) -> Vec<(&'static str, Out<'_>)> {
    let (plane, lists) = sketch_lists(sketch);
    let mut fields = vec![("plane", Out::Str(plane.as_str()))];
    fields.extend(
        lists
            .into_iter()
            .map(|(kind, items)| (kind.list(), Out::Array(items))),
    );
    fields
}

/// Each of a sketch's points, lines, circles, arcs, and constraints, in the order the file
/// writes them, as its kind (`point`, ...) and its one line of canonical JSON: for an
/// outline, where an edit to one element changes its line alone.
pub(crate) fn sketch_elements(sketch: &Sketch) -> Vec<(&'static str, String)> {
    let (_, lists) = sketch_lists(sketch);
    let mut elements = Vec::new();
    for (kind, items) in lists {
        for item in &items {
            let mut writer = Writer::new(usize::MAX, true);
            // With no limit, writing can't fail: it fails only past its limit.
            let _ = writer.put(item, 0);
            elements.push((kind.noun(), writer.out));
        }
    }
    elements
}

/// A constraint: its ID, its type, then its form's fields in schema order.
fn constraint(constraint: &SketchConstraint) -> Out<'_> {
    let mut members = vec![
        ("id", element(&constraint.id)),
        ("type", Out::Str(constraint.form.type_name())),
    ];
    let length = |l: &Length| number(*l);
    let angle = |a: &Angle| Out::Raw(a.to_string());
    let driving = |d: &bool| boolean(*d);
    members.extend(match &constraint.form {
        Form::Horizontal { line } | Form::Vertical { line } => vec![("line", r(line))],
        Form::HorizontalPoints { a, b }
        | Form::VerticalPoints { a, b }
        | Form::Parallel { a, b }
        | Form::Perpendicular { a, b }
        | Form::Equal { a, b } => vec![("a", r(a)), ("b", r(b))],
        Form::Length {
            line,
            value,
            driving: d,
        } => vec![
            ("line", r(line)),
            ("value", length(value)),
            ("driving", driving(d)),
        ],
        Form::Distance {
            a,
            b,
            value,
            driving: d,
        }
        | Form::DistanceX {
            a,
            b,
            value,
            driving: d,
        }
        | Form::DistanceY {
            a,
            b,
            value,
            driving: d,
        } => vec![
            ("a", r(a)),
            ("b", r(b)),
            ("value", length(value)),
            ("driving", driving(d)),
        ],
        Form::PointLineDistance {
            point,
            line,
            value,
            side,
            driving: d,
        } => vec![
            ("point", r(point)),
            ("line", r(line)),
            ("value", length(value)),
            ("side", Out::Str(side.as_str())),
            ("driving", driving(d)),
        ],
        Form::On { point, curve } => vec![("point", r(point)), ("curve", r(curve))],
        Form::Angle {
            a,
            b,
            value,
            driving: d,
        } => vec![
            ("a", r(a)),
            ("b", r(b)),
            ("value", angle(value)),
            ("driving", driving(d)),
        ],
        Form::Radius {
            curve,
            value,
            driving: d,
        }
        | Form::Diameter {
            curve,
            value,
            driving: d,
        } => vec![
            ("curve", r(curve)),
            ("value", length(value)),
            ("driving", driving(d)),
        ],
        Form::Tangent { a, b, side } => {
            vec![("a", r(a)), ("b", r(b)), ("side", Out::Str(side.as_str()))]
        }
        Form::TangentOn {
            a,
            at,
            b,
            direction,
        } => vec![
            ("a", r(a)),
            ("at", r(at)),
            ("b", r(b)),
            ("direction", Out::Str(direction.as_str())),
        ],
        Form::TangentAt {
            a,
            b,
            at,
            direction,
        } => vec![
            ("a", r(a)),
            ("b", r(b)),
            ("at", r(at)),
            ("direction", Out::Str(direction.as_str())),
        ],
        Form::Symmetric { a, b, about } => {
            vec![("a", r(a)), ("b", r(b)), ("about", r(about))]
        }
        Form::Midpoint { point, line } => vec![("point", r(point)), ("line", r(line))],
    });
    Out::Object(members)
}

fn r(reference: &Reference) -> Out<'_> {
    Out::Str(reference.as_str())
}

fn element(id: &ElementId) -> Out<'_> {
    Out::Str(id.as_str())
}

fn coordinate(c: Coordinate) -> Out<'static> {
    Out::Raw(c.to_string())
}

fn boolean(b: bool) -> Out<'static> {
    Out::Raw(if b { "true" } else { "false" }.to_owned())
}

fn number(length: Length) -> Out<'static> {
    Out::Raw(length.to_string())
}

fn point(position: &Position) -> Out<'static> {
    Out::Array(position.iter().map(|&l| number(l)).collect())
}

/// Element names, one to a line, so an edit to one is a line of the diff.
fn names(names: &[ElementName]) -> Out<'_> {
    Out::Lines(names.iter().map(|n| Out::Str(n.as_str())).collect())
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
            Out::Array(items) | Out::Lines(items) => {
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
