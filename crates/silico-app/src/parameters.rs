//! The selected feature's PARAMETERS (app.js's `featureProps`, its `rows`): what the property
//! panel says of a feature's own values, each a row of a key and its value, as the
//! prototype lays them out: a sketch's plane, definition, curves, and dimensions; a pad's
//! profile, extent, and direction; a pocket's profile, extent, and base; a fillet's edges,
//! radius, and base. The app's other kinds the prototype draws none of (a revolve, a
//! groove, a chamfer, a boolean) are given the rows of the kind they're like: a revolve's
//! a pad's, with its axis; a groove's a pocket's, with its axis; a chamfer's a fillet's,
//! its distance for the radius; a boolean's its target and tool, as its dialog names them.
//! A primitive, which the app makes none of (Trevin's decision of 2026-11-09), has none.
//!
//! A sketch's rows are its read's, the one the browser's prefix and the viewport's lines
//! are of ([`crate::sketch::SketchReads`]). Every other feature's are its text's, as the
//! document holds it at the tab's revision ([`FeatureRead`]: `text` of the feature, at that
//! revision by number, so the read never reads the file and never reloads it), read off
//! the UI thread as its dialog's is, and taken apart as its dialog takes it.

use eframe::egui::Color32;
use silico_core::command::sketch::SketchRead;
use silico_core::command::wire::Named;
use silico_core::command::{Revision, Running, SessionId, Sessions, TextAnswer};
use silico_core::document::json::{Node, Value};
use silico_core::document::{ANGLE_DECIMALS, Angle, FeatureId, Form, Length, Plane};
use silico_core::kernel::{Clients, SketchPlane};
use silico_core::sketch_states::SketchState;

use crate::theme;

/// One row's value: words in a colour, or a link to a feature (its name, which selects it)
/// with words after it.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Text(String, Color32),
    Link {
        feature: FeatureId,
        name: String,
        after: Option<String>,
    },
}

/// One row: its key and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: &'static str,
    pub value: Shown,
}

fn text(key: &'static str, words: impl Into<String>) -> Row {
    Row {
        key,
        value: Shown::Text(words.into(), theme::PROPERTY_VALUE),
    }
}

/// Whether a feature of the type `kind` has PARAMETERS: so the section stands while its
/// rows are read.
pub fn has_parameters(kind: &str) -> bool {
    matches!(
        kind,
        "sketch"
            | "pad"
            | "pocket"
            | "revolve"
            | "groove"
            | "fillet"
            | "chamfer"
            | "fuse"
            | "cut"
            | "common"
    )
}

/// The feature's text as the document holds it at a revision, read for the selected
/// feature, and the read in flight. What's held stays until the next read answers, so a
/// new revision's values replace the last's with nothing between them.
#[derive(Default)]
pub struct FeatureRead {
    held: Option<Held>,
    reading: Option<(FeatureId, Revision, Running<TextAnswer>)>,
    /// How many reads it has sent: what a test counts.
    pub reads: u64,
}

/// A feature's text read: the feature, the revision it's of, and its tree, or why it has
/// none (the read refused: a revision the session no longer holds, the feature gone).
struct Held {
    feature: FeatureId,
    revision: Revision,
    node: Option<Node>,
}

impl FeatureRead {
    /// The read in flight polled, without waiting; then, if what's held or in flight isn't
    /// the feature `feature`'s text at `revision`, it read, at that revision by number.
    pub fn poll<P: Clients + 'static>(
        &mut self,
        sessions: &Sessions<P>,
        session: &SessionId,
        feature: &FeatureId,
        revision: Revision,
    ) {
        if let Some((id, asked, running)) = self.reading.take() {
            match running.try_take() {
                Ok(response) => {
                    let node = response.result.ok().and_then(|answer| {
                        silico_core::document::json::read(answer.text.as_bytes()).ok()
                    });
                    self.held = Some(Held {
                        feature: id,
                        revision: asked,
                        node,
                    });
                }
                Err(running) => self.reading = Some((id, asked, running)),
            }
        }
        let asked = |id: &FeatureId, at: Revision| id == feature && at == revision;
        let read = self
            .held
            .as_ref()
            .is_some_and(|held| asked(&held.feature, held.revision));
        let in_flight = self
            .reading
            .as_ref()
            .is_some_and(|(id, at, _)| asked(id, *at));
        if read || in_flight {
            return;
        }
        // One read at a time: an older one still out is let go, its answer never taken.
        self.reads += 1;
        let running = sessions.text_at(
            session,
            Some(feature.clone()),
            Some(Named::Revision(revision)),
        );
        self.reading = Some((feature.clone(), revision, running));
    }

    /// The feature `feature`'s text, as last read, if it's read and was there.
    pub fn node(&self, feature: &FeatureId) -> Option<&Node> {
        self.held
            .as_ref()
            .filter(|held| &held.feature == feature)
            .and_then(|held| held.node.as_ref())
    }
}

/// What a feature's rows read besides its own text: the features' names, and the sketches'
/// reads.
pub struct Context<'a> {
    pub name: &'a dyn Fn(&FeatureId) -> String,
    pub sketch: &'a dyn Fn(&FeatureId) -> Option<&'a SketchRead>,
}

/// The rows of a sketch, from its read (`read`) and its state (`state`, sketch mode's for
/// the sketch last open there): its plane as the browser names it, its definition in the
/// canvas's colours, its curves counted by kind, and its driving dimensions' values. None
/// before it's read (a suppressed sketch is never read).
pub fn sketch_rows(read: Option<&SketchRead>, state: Option<SketchState>) -> Vec<Row> {
    let Some(read) = read else {
        return Vec::new();
    };
    let sketch = &read.members;
    let mut rows = vec![text("Plane", plane_words(&sketch.plane))];
    if let Some(state) = state {
        let (words, colour) = match state {
            SketchState::UnderDefined => ("Under defined", theme::SKETCH_UNDER_DEFINED),
            SketchState::FullyDefined => ("Fully defined", theme::PROPERTY_VALUE),
            SketchState::OverDefined => ("Over defined", theme::ERROR_TEXT),
            SketchState::NoSolution => ("No solution found", theme::ERROR_TEXT),
            SketchState::Invalid => ("Invalid solution found", theme::ERROR_TEXT),
        };
        rows.push(Row {
            key: "Definition",
            value: Shown::Text(words.to_owned(), colour),
        });
    }
    let counts = [
        (sketch.lines.len(), "line", "lines"),
        (sketch.arcs.len(), "arc", "arcs"),
        (sketch.circles.len(), "circle", "circles"),
    ];
    let curves: Vec<String> = counts
        .into_iter()
        .filter(|(count, ..)| *count > 0)
        .map(|(count, one, many)| counted(count, one, many))
        .collect();
    rows.push(text("Curves", none_or(curves.join(", "))));
    rows.push(text("Dimensions", none_or(dimensions(sketch))));
    rows
}

/// `words`, or "None" if there are none.
fn none_or(words: String) -> String {
    if words.is_empty() {
        "None".to_owned()
    } else {
        words
    }
}

/// `count` of a thing, `one` or `many` as it takes.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A sketch's plane, as the browser's meta names it: an origin plane by its name, a planar
/// face as `Face`.
fn plane_words(plane: &Plane) -> &'static str {
    use silico_core::document::OriginPlane;
    match plane {
        Plane::Origin(OriginPlane::Xy) => "XY",
        Plane::Origin(OriginPlane::Xz) => "XZ",
        Plane::Origin(OriginPlane::Yz) => "YZ",
        Plane::Face(_) => "Face",
    }
}

/// The sketch's driving dimensions' values, in the order it holds them, as the prototype
/// writes its bracket's (`40, 30, 6, 8 mm`): the lengths with their unit after the last,
/// then the angles, each with its degree sign; reference dimensions left out, as they
/// define nothing.
fn dimensions(sketch: &silico_core::document::Sketch) -> String {
    let driving = || {
        sketch
            .constraints
            .iter()
            .filter(|constraint| constraint.form.driving() == Some(true))
    };
    let lengths: Vec<String> = driving()
        .filter_map(|constraint| constraint.form.length())
        .map(|(value, _)| mm(value))
        .collect();
    let angles: Vec<String> = driving()
        .filter_map(|constraint| match &constraint.form {
            Form::Angle { value, .. } => Some(degrees(*value)),
            _ => None,
        })
        .collect();
    let mut parts = Vec::new();
    if !lengths.is_empty() {
        parts.push(format!("{} mm", lengths.join(", ")));
    }
    if !angles.is_empty() {
        parts.push(angles.join(", "));
    }
    parts.join(" · ")
}

/// A length's millimetres, as the prototype's `fmt` writes them: at most two decimals.
fn mm(length: Length) -> String {
    crate::properties::trimmed(i128::from(length.nm()), crate::properties::NM_PER_MM)
}

/// An angle's degrees, as the prototype's `fmt` writes them, with its sign.
fn degrees(angle: Angle) -> String {
    let per = 10_i128.pow(ANGLE_DECIMALS);
    format!(
        "{}°",
        crate::properties::trimmed(i128::from(angle.ndeg()), per)
    )
}

/// `node`'s member `key`, if it's an object holding one.
fn member<'a>(node: &'a Node, key: &str) -> Option<&'a Node> {
    match &node.value {
        Value::Object(members) => members
            .iter()
            .find(|member| member.key == key)
            .map(|member| &member.value),
        _ => None,
    }
}

fn string<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    match &member(node, key)?.value {
        Value::String(text) => Some(text),
        _ => None,
    }
}

fn feature(node: &Node, key: &str) -> Option<FeatureId> {
    FeatureId::new(string(node, key)?)
}

fn length(node: &Node, key: &str) -> Option<Length> {
    match &member(node, key)?.value {
        Value::Number(text) => Length::parse(text).ok(),
        _ => None,
    }
}

fn angle(node: &Node, key: &str) -> Option<Angle> {
    match &member(node, key)?.value {
        Value::Number(text) => Angle::parse(text).ok(),
        _ => None,
    }
}

fn reversed(node: &Node) -> Option<bool> {
    match &member(node, "reversed")?.value {
        Value::Bool(on) => Some(*on),
        _ => None,
    }
}

/// The rows of a feature of the type `kind` other than a sketch, from `node`, its text as
/// the document holds it; none for a kind with no PARAMETERS ([`has_parameters`]), and a
/// row left out where its value isn't there.
pub fn feature_rows(kind: &str, node: &Node, context: &Context<'_>) -> Vec<Row> {
    let mut rows = Vec::new();
    let base = || feature(node, "base").map(|base| text("Base", (context.name)(&base)));
    match kind {
        "pad" | "pocket" | "revolve" | "groove" => {
            let sketch = feature(node, "sketch");
            let read = sketch.as_ref().and_then(|sketch| (context.sketch)(sketch));
            if let Some(sketch) = &sketch {
                rows.push(profile(sketch, read, context));
            }
            if matches!(kind, "revolve" | "groove")
                && let Some(axis) = string(node, "axis")
            {
                rows.push(text("Axis", axis_words(axis)));
            }
            let extent = member(node, "extent");
            if let Some(words) = extent.and_then(extent_words) {
                rows.push(text("Extent", words));
            }
            let flips = extent.and_then(|extent| {
                (string(extent, "type") == Some("one_side")).then(|| reversed(extent))?
            });
            match kind {
                "pad" | "revolve" => {
                    if let Some(reversed) = flips {
                        // The way a pad sweeps, along its sketch's normal or against it, the
                        // normal's axis named once the sketch is read (`Normal (−Y)`); a
                        // turn's sense has no axis to name.
                        let normal = (kind == "pad")
                            .then(|| read.and_then(|read| read.plane.as_ref().ok()))
                            .flatten()
                            .copied();
                        rows.push(text("Direction", direction_words(reversed, normal)));
                    }
                }
                _ => rows.extend(base()),
            }
        }
        "fillet" | "chamfer" => {
            if let Some(Value::Array(edges)) = member(node, "edges").map(|edges| &edges.value) {
                rows.push(text("Edges", edges.len().to_string()));
            }
            let size = if kind == "fillet" {
                length(node, "radius").map(|radius| ("Radius", radius))
            } else {
                member(node, "size")
                    .and_then(|size| length(size, "distance"))
                    .map(|distance| ("Distance", distance))
            };
            if let Some((key, value)) = size {
                rows.push(text(key, format!("{} mm", mm(value))));
            }
            rows.extend(base());
        }
        "fuse" | "cut" | "common" => {
            for (key, field) in [("Target Body", "base"), ("Tool Body", "tool")] {
                if let Some(id) = feature(node, field) {
                    rows.push(text(key, (context.name)(&id)));
                }
            }
        }
        _ => {}
    }
    rows
}

/// A sweep's Profile row: its sketch, a link that selects it, and once the sketch is read,
/// how many regions its profile has (`Bracket profile · 1 region`).
fn profile(sketch: &FeatureId, read: Option<&SketchRead>, context: &Context<'_>) -> Row {
    Row {
        key: "Profile",
        value: Shown::Link {
            feature: sketch.clone(),
            name: (context.name)(sketch),
            after: read
                .map(|read| format!("· {}", counted(read.regions.len(), "region", "regions"))),
        },
    }
}

/// An extent as the prototype words a pad's (`Distance · 12 mm`) and a pocket's (`Through
/// all`): its form, then its values.
fn extent_words(extent: &Node) -> Option<String> {
    Some(match string(extent, "type")? {
        "one_side" => match (length(extent, "length"), angle(extent, "angle")) {
            (Some(length), _) => format!("Distance · {} mm", mm(length)),
            (None, Some(angle)) => format!("Angle · {}", degrees(angle)),
            (None, None) => return None,
        },
        "symmetric" => match (length(extent, "length"), angle(extent, "angle")) {
            (Some(length), _) => format!("Symmetric · {} mm", mm(length)),
            (None, Some(angle)) => format!("Symmetric · {}", degrees(angle)),
            (None, None) => return None,
        },
        "two_sides" => format!(
            "Two sides · {} mm, {} mm",
            mm(length(extent, "length")?),
            mm(length(extent, "length2")?)
        ),
        "through_all" => "Through all".to_owned(),
        _ => return None,
    })
}

/// A turn's axis, as its dialog names it: the sketch's X or Y axis, or a line of it by its
/// ID.
fn axis_words(axis: &str) -> String {
    match axis {
        "x_axis" => "X axis".to_owned(),
        "y_axis" => "Y axis".to_owned(),
        line => line.to_owned(),
    }
}

/// A one-sided sweep's way (`Normal (−Y)`, `Reversed (+Y)`): along its sketch's normal, or
/// against it if `reversed`, the way named as a signed axis where it's one (the minus a
/// true minus, as the prototype's), else as the prototype writes a direction, and not at
/// all before the sketch is placed.
fn direction_words(reversed: bool, normal: Option<SketchPlane>) -> String {
    let word = if reversed { "Reversed" } else { "Normal" };
    let Some(plane) = normal else {
        return word.to_owned();
    };
    let way = plane
        .normal()
        .map(|component| if reversed { -component } else { component } + 0.0);
    format!("{word} ({})", way_words(way))
}

/// `way` as a signed axis (`+X`, `−Y`) if it's one exactly, else its components.
fn way_words(way: [f64; 3]) -> String {
    let axes = ["X", "Y", "Z"];
    let along: Vec<usize> = (0..3).filter(|&at| way[at] != 0.0).collect();
    match along.as_slice() {
        [at] if way[*at].abs() == 1.0 => {
            let sign = if way[*at] > 0.0 { "+" } else { "\u{2212}" };
            format!("{sign}{}", axes[*at])
        }
        _ => {
            let written = crate::properties::direction(way);
            written
                .strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .unwrap_or(&written)
                .to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(text: &str) -> Node {
        silico_core::document::json::read(text.as_bytes()).unwrap()
    }

    fn named(id: &FeatureId) -> String {
        match id.as_str() {
            "s0000001" => "Bracket profile".to_owned(),
            "b0000001" => "Upright".to_owned(),
            "t0000001" => "Pin".to_owned(),
            other => other.to_owned(),
        }
    }

    fn no_sketch(_: &FeatureId) -> Option<&'static SketchRead> {
        None
    }

    fn rows(kind: &str, text: &str) -> Vec<(&'static str, String)> {
        let context = Context {
            name: &named,
            sketch: &no_sketch,
        };
        feature_rows(kind, &tree(text), &context)
            .into_iter()
            .map(|row| match row.value {
                Shown::Text(words, _) => (row.key, words),
                Shown::Link { name, after, .. } => (
                    row.key,
                    format!(
                        "[{name}]{}",
                        after.map(|after| format!(" {after}")).unwrap_or_default()
                    ),
                ),
            })
            .collect()
    }

    #[test]
    fn a_pads_rows_are_its_profile_its_extent_and_its_direction() {
        // app.js's `featureProps`: Profile, Extent (`Distance · 12 mm`), Direction.
        let pad = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 12, "reversed": false}}"#;
        assert_eq!(
            rows("pad", pad),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Extent", "Distance · 12 mm".to_owned()),
                ("Direction", "Normal".to_owned()),
            ]
        );
        let flipped = pad.replace("false", "true");
        assert_eq!(
            rows("pad", &flipped)[2],
            ("Direction", "Reversed".to_owned())
        );
        // A symmetric pad has no way to flip: no Direction.
        let symmetric = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "symmetric", "length": 10.125}}"#;
        assert_eq!(
            rows("pad", symmetric),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                // `fmt`'s two decimals, half away from zero.
                ("Extent", "Symmetric · 10.13 mm".to_owned()),
            ]
        );
        let two = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "two_sides", "length": 5, "length2": 2.5}}"#;
        assert_eq!(
            rows("pad", two)[1],
            ("Extent", "Two sides · 5 mm, 2.5 mm".to_owned())
        );
    }

    #[test]
    fn a_pockets_rows_are_its_profile_its_extent_and_its_base() {
        let pocket = r#"{"id": "q0000001", "type": "pocket", "v": 1, "name": "Holes", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "through_all", "reversed": false}}"#;
        assert_eq!(
            rows("pocket", pocket),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Extent", "Through all".to_owned()),
                ("Base", "Upright".to_owned()),
            ]
        );
    }

    #[test]
    fn a_fillets_rows_are_its_edges_its_radius_and_its_base_and_a_chamfers_alike() {
        let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "b0000001", "edges": ["b0000001.pad.Edge3", "b0000001.pad.Edge7"], "radius": 1.5}"#;
        assert_eq!(
            rows("fillet", fillet),
            [
                ("Edges", "2".to_owned()),
                ("Radius", "1.5 mm".to_owned()),
                ("Base", "Upright".to_owned()),
            ]
        );
        let chamfer = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "b0000001", "edges": ["b0000001.pad.Edge3"], "size": {"distance": 0.5}}"#;
        assert_eq!(
            rows("chamfer", chamfer),
            [
                ("Edges", "1".to_owned()),
                ("Distance", "0.5 mm".to_owned()),
                ("Base", "Upright".to_owned()),
            ]
        );
    }

    #[test]
    fn a_turns_rows_name_its_axis_and_its_angle() {
        let revolve = r#"{"id": "r0000001", "type": "revolve", "v": 1, "name": "Turn", "sketch": "s0000001", "axis": "y_axis", "extent": {"type": "one_side", "angle": 270, "reversed": true}}"#;
        assert_eq!(
            rows("revolve", revolve),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Axis", "Y axis".to_owned()),
                ("Extent", "Angle · 270°".to_owned()),
                ("Direction", "Reversed".to_owned()),
            ]
        );
        let groove = r#"{"id": "g0000001", "type": "groove", "v": 1, "name": "Ring", "base": "b0000001", "sketch": "s0000001", "axis": "na000001", "extent": {"type": "symmetric", "angle": 90}}"#;
        assert_eq!(
            rows("groove", groove),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Axis", "na000001".to_owned()),
                ("Extent", "Symmetric · 90°".to_owned()),
                ("Base", "Upright".to_owned()),
            ]
        );
    }

    #[test]
    fn a_booleans_rows_are_its_target_and_its_tool() {
        let cut = r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "b0000001", "tool": "t0000001"}"#;
        assert_eq!(
            rows("cut", cut),
            [
                ("Target Body", "Upright".to_owned()),
                ("Tool Body", "Pin".to_owned()),
            ]
        );
        // A primitive has none.
        let block = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
        assert_eq!(rows("box", block), []);
        assert!(!has_parameters("box"));
    }

    #[test]
    fn a_ways_axis_is_signed_with_a_true_minus_and_an_oblique_one_written_whole() {
        // SketchPlane's normals: XY's +Z, XZ's −Y (the prototype's bracket's `Normal (−Y)`).
        assert_eq!(
            direction_words(false, Some(SketchPlane::Xz)),
            "Normal (\u{2212}Y)"
        );
        assert_eq!(
            direction_words(true, Some(SketchPlane::Xz)),
            "Reversed (+Y)"
        );
        assert_eq!(direction_words(false, Some(SketchPlane::Xy)), "Normal (+Z)");
        assert_eq!(
            direction_words(true, Some(SketchPlane::Yz)),
            "Reversed (\u{2212}X)"
        );
        assert_eq!(way_words([0.6, 0.0, 0.8]), "0.6, 0, 0.8");
    }
}
