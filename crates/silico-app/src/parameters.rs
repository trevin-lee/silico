//! The selected feature's PARAMETERS (app.js's `featureProps`, its `rows`): what the property
//! panel says of a feature's own values, each a row of a key and its value, as the
//! prototype lays them out: a sketch's plane, definition, curves, and dimensions; a pad's
//! profile, extent, and direction; a pocket's profile, extent, and base; a fillet's edges,
//! radius, and base. Every stored value that changes the solid has its row: a pocket's
//! direction too, and the base of a pad or revolve that has one. The app's other kinds the
//! prototype draws none of (a revolve, a groove, a chamfer, a boolean) are given the rows of
//! the kind they're like: a revolve's a pad's, with its axis and its turning sense; a
//! groove's a pocket's, with its axis; a chamfer's a fillet's, its distance for the radius;
//! a boolean's its target and tool, as its dialog names them. A primitive, which the app
//! makes none of (Trevin's decision of 2026-11-09), has none.
//!
//! A sketch's rows are its read's, the one the browser's prefix and the viewport's lines
//! are of ([`crate::sketch::SketchReads`]). Every other feature's are its text's, as the
//! document holds it at the tab's revision ([`FeatureRead`]: `text` of the feature, at that
//! revision by number, so the read never reads the file and never reloads it), read off
//! the UI thread as its dialog's is; its extent read by the core's reader
//! ([`read_extent`]), and a sweep's way by the core's rule ([`sweep::prism_direction`]), as
//! the dialog's arrows read them (`crate::arrow`).

use eframe::egui::Color32;
use silico_core::command::sketch::SketchRead;
use silico_core::command::wire::Named;
use silico_core::command::{Revision, Running, SessionId, Sessions, TextAnswer};
use silico_core::document::json::{Node, Value};
use silico_core::document::{
    ANGLE_DECIMALS, Angle, Extent, FeatureId, Length, PadExtent, PocketExtent, SweepFeature,
    TurnExtent, read_extent,
};
use silico_core::kernel::{Clients, SketchPlane};
use silico_core::sketch_states::SketchState;
use silico_core::sweep;

use crate::dialog::member;
use crate::theme;

/// One row's value: words in a colour, an ID in the monospace type (DESIGN.md, *Type*), or
/// a link to a feature (its name, which selects it) with words after it.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Text(String, Color32),
    Id(String),
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

/// Whether a feature of the type `kind` may have PARAMETERS, so its text is read for them.
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
/// new revision's values replace the last's with nothing between them; a read refused once
/// it ran (a revision the session no longer holds, the feature gone) keeps the feature's
/// last text and isn't sent again for that revision; one refused before it ran (the
/// session not to be read) is sent again by a later poll, as a sketch's read is.
#[derive(Default)]
pub struct FeatureRead {
    held: Option<Held>,
    /// The feature and revision the last answer was for, whatever it said.
    answered: Option<(FeatureId, Revision)>,
    reading: Option<(FeatureId, Revision, Running<TextAnswer>)>,
    /// How many reads it has sent: what a test counts.
    pub reads: u64,
}

/// A feature's text read: the feature, and its tree.
struct Held {
    feature: FeatureId,
    node: Node,
}

impl FeatureRead {
    /// The read in flight polled, without waiting; then, if neither the last answer nor the
    /// read in flight is the feature `feature`'s text at `revision`, it read, at that
    /// revision by number.
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
                    let ran = response.session.is_some();
                    let node = response.result.ok().and_then(|answer| {
                        silico_core::document::json::read(answer.text.as_bytes()).ok()
                    });
                    if let Some(node) = node {
                        self.held = Some(Held {
                            feature: id.clone(),
                            node,
                        });
                    }
                    if ran {
                        self.answered = Some((id, asked));
                    }
                }
                Err(running) => self.reading = Some((id, asked, running)),
            }
        }
        let asked = |id: &FeatureId, at: Revision| id == feature && at == revision;
        let answered = self
            .answered
            .as_ref()
            .is_some_and(|(id, at)| asked(id, *at));
        let in_flight = self
            .reading
            .as_ref()
            .is_some_and(|(id, at, _)| asked(id, *at));
        if answered || in_flight {
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

    /// `node` held as the feature `feature`'s text, as a read's answer holds it: a test's
    /// stand-in for a session's.
    #[cfg(test)]
    pub(crate) fn hold(&mut self, feature: FeatureId, node: Node) {
        self.held = Some(Held { feature, node });
    }

    /// The feature `feature`'s text, as last read, if one was.
    pub fn node(&self, feature: &FeatureId) -> Option<&Node> {
        self.held
            .as_ref()
            .filter(|held| &held.feature == feature)
            .map(|held| &held.node)
    }
}

/// What a feature's rows read besides its own text: the features' names, and the sketches'
/// reads.
pub struct Context<'a> {
    pub name: &'a dyn Fn(&FeatureId) -> String,
    pub sketch: &'a dyn Fn(&FeatureId) -> Option<&'a SketchRead>,
}

/// The rows of a sketch, from its read: its plane as the browser names it; its definition
/// in sketch mode's words and colours ([`crate::sketch::state_shown`]), but Fully defined
/// in the value's own Gray.800, as `featureProps` leaves it; its curves, construction left
/// out, counted by kind, and the circles' diameter if they share one (`2 circles, ⌀ 5.5
/// mm`); and its driving dimensions' values. None before it's read (a suppressed sketch is
/// never read).
pub fn sketch_rows(read: Option<&SketchRead>) -> Vec<Row> {
    let Some(read) = read else {
        return Vec::new();
    };
    let sketch = &read.members;
    let mut rows = vec![text("Plane", crate::panes::plane_meta(&sketch.plane))];
    let shown = crate::sketch::state_shown(read);
    let empty = sketch.elements().next().is_none();
    let colour = if read.states.state == SketchState::FullyDefined && !empty {
        theme::PROPERTY_VALUE
    } else {
        shown.colour
    };
    rows.push(Row {
        key: "Definition",
        value: Shown::Text(shown.words, colour),
    });
    let lines = sketch
        .lines
        .iter()
        .filter(|line| !line.construction)
        .count();
    let arcs = sketch.arcs.iter().filter(|arc| !arc.construction).count();
    let circles: Vec<_> = sketch
        .circles
        .iter()
        .filter(|circle| !circle.construction)
        .collect();
    let counts = [
        (lines, "line", "lines"),
        (arcs, "arc", "arcs"),
        (circles.len(), "circle", "circles"),
    ];
    let mut curves: Vec<String> = counts
        .into_iter()
        .filter(|(count, ..)| *count > 0)
        .map(|(count, one, many)| counted(count, one, many))
        .collect();
    let diameters: Vec<Option<Length>> = circles
        .iter()
        .map(|circle| Length::rounded(2.0 * circle.radius.mm()))
        .collect();
    if let [Some(first), rest @ ..] = diameters.as_slice()
        && rest.iter().all(|other| *other == Some(*first))
    {
        curves.push(format!("⌀ {} mm", mm(*first)));
    }
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

/// The sketch's driving dimensions' values, in the order it holds them, as the prototype
/// writes its bracket's (`40, 30, 6, 8 mm`): the lengths with their unit after the last, a
/// radius and a diameter marked as the canvas marks them (`R5`, `Ø11`), then the angles,
/// each with its degree sign; reference dimensions left out, as they define nothing.
fn dimensions(sketch: &silico_core::document::Sketch) -> String {
    use silico_core::document::Form;
    let driving = || {
        sketch
            .constraints
            .iter()
            .filter(|constraint| constraint.form.driving() == Some(true))
            .map(|constraint| &constraint.form)
    };
    let lengths: Vec<String> = driving()
        .filter_map(|form| {
            let (value, _) = form.length()?;
            Some(match form {
                Form::Radius { .. } => format!("R{}", mm(value)),
                Form::Diameter { .. } => format!("Ø{}", mm(value)),
                _ => mm(value),
            })
        })
        .collect();
    let angles: Vec<String> = driving()
        .filter_map(|form| match form {
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

fn string<'a>(node: &'a Node, key: &str) -> Option<&'a str> {
    match &member(node, key)?.value {
        Value::String(text) => Some(text),
        _ => None,
    }
}

fn feature(node: &Node, key: &str) -> Option<FeatureId> {
    FeatureId::new(string(node, key)?)
}

/// `node`'s length `key`, read by the core's number reader.
fn length(node: &Node, key: &str) -> Option<Length> {
    match &member(node, key)?.value {
        Value::Number(text) => Length::parse(text).ok(),
        _ => None,
    }
}

/// The extent `node` holds, of a feature of `kind`, read as the core reads a document's
/// (`read_extent`, of its text written back from its tree): none if it doesn't read, as
/// the core would refuse it.
fn extent_of(kind: SweepFeature, node: &Node) -> Option<Extent> {
    let Value::Object(members) = &node.value else {
        return None;
    };
    let mut written = Vec::with_capacity(members.len());
    for member in members {
        let value = match &member.value.value {
            Value::Number(text) => text.clone(),
            Value::String(text) => crate::dialog::quoted(text),
            Value::Bool(on) => on.to_string(),
            Value::Null => "null".to_owned(),
            // An extent's values are numbers, a string, and a flag: anything else the
            // core's reader refuses too.
            Value::Array(_) | Value::Object(_) => return None,
        };
        written.push(format!("{}: {value}", crate::dialog::quoted(&member.key)));
    }
    read_extent(kind, &format!("{{{}}}", written.join(", "))).ok()
}

/// The rows of a feature of the type `kind` other than a sketch, from `node`, its text as
/// the document holds it; none for a kind with no PARAMETERS ([`has_parameters`]), and a
/// row left out where its value isn't there or doesn't read.
pub fn feature_rows(kind: &str, node: &Node, context: &Context<'_>) -> Vec<Row> {
    let mut rows = Vec::new();
    let base = feature(node, "base").map(|base| text("Base", (context.name)(&base)));
    let sweep_kind = match kind {
        "pad" => Some(SweepFeature::Pad),
        "pocket" => Some(SweepFeature::Pocket),
        "revolve" => Some(SweepFeature::Revolve),
        "groove" => Some(SweepFeature::Groove),
        _ => None,
    };
    if let Some(sweeps) = sweep_kind {
        let sketch = feature(node, "sketch");
        let read = sketch.as_ref().and_then(|sketch| (context.sketch)(sketch));
        if let Some(sketch) = &sketch {
            rows.push(profile(sketch, read, context));
        }
        if let Some(axis) = string(node, "axis") {
            rows.push(axis_row(axis));
        }
        let extent = member(node, "extent").and_then(|extent| extent_of(sweeps, extent));
        if let Some(extent) = extent {
            rows.push(text("Extent", extent_words(extent)));
            let normal = read
                .and_then(|read| read.plane.as_ref().ok())
                .map(|plane| SketchPlane::normal(*plane));
            if let Some(words) = direction_words(sweeps, extent, normal) {
                rows.push(text("Direction", words));
            }
        }
        rows.extend(base);
        return rows;
    }
    match kind {
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
            rows.extend(base);
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
fn extent_words(extent: Extent) -> String {
    match extent {
        Extent::Pad(PadExtent::OneSide { length, .. })
        | Extent::Pocket(PocketExtent::OneSide { length, .. }) => {
            format!("Distance · {} mm", mm(length))
        }
        Extent::Pad(PadExtent::Symmetric { length })
        | Extent::Pocket(PocketExtent::Symmetric { length }) => {
            format!("Symmetric · {} mm", mm(length))
        }
        Extent::Pad(PadExtent::TwoSides { length, length2 })
        | Extent::Pocket(PocketExtent::TwoSides { length, length2 }) => {
            format!("Two sides · {} mm, {} mm", mm(length), mm(length2))
        }
        Extent::Pocket(PocketExtent::ThroughAll { .. }) => "Through all".to_owned(),
        Extent::Turn(TurnExtent::OneSide { angle, .. }) => format!("Angle · {}", degrees(angle)),
        Extent::Turn(TurnExtent::Symmetric { angle }) => {
            format!("Symmetric · {}", degrees(angle))
        }
    }
}

/// A turn's axis, as its dialog names it: the sketch's X or Y axis, or a line of it by its
/// ID, in the monospace type.
fn axis_row(axis: &str) -> Row {
    let value = match axis {
        "x_axis" => Shown::Text("X axis".to_owned(), theme::PROPERTY_VALUE),
        "y_axis" => Shown::Text("Y axis".to_owned(), theme::PROPERTY_VALUE),
        line => Shown::Id(line.to_owned()),
    };
    Row { key: "Axis", value }
}

/// The way a sweep goes, for an extent whose form says one (`reversed`): a pad's or a
/// pocket's `Normal (−Y)` or `Reversed (+Y)`, the way the core's rule gives for the stored
/// flag ([`sweep::prism_direction`]: a pad along its sketch's normal, a pocket against it)
/// as a signed axis where it's one (the minus a true minus, as the prototype's), else as
/// the prototype writes a direction, and not at all before the sketch is placed; a turn's
/// sense, right-handed about its axis, or left-handed reversed. None for a form with no
/// way to flip.
fn direction_words(kind: SweepFeature, extent: Extent, normal: Option<[f64; 3]>) -> Option<String> {
    let reversed = match extent {
        Extent::Pad(PadExtent::OneSide { reversed, .. })
        | Extent::Pocket(
            PocketExtent::OneSide { reversed, .. } | PocketExtent::ThroughAll { reversed },
        ) => reversed,
        Extent::Turn(TurnExtent::OneSide { reversed, .. }) => {
            return Some(
                if reversed {
                    "Left-handed"
                } else {
                    "Right-handed"
                }
                .to_owned(),
            );
        }
        _ => return None,
    };
    let word = if reversed { "Reversed" } else { "Normal" };
    Some(match normal {
        Some(normal) => {
            let way = sweep::prism_direction(kind, reversed, normal);
            format!("{word} ({})", way_words(way))
        }
        None => word.to_owned(),
    })
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
    use silico_core::document::Operation;
    use silico_core::sketch_states::SketchStates;

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

    /// Each row as its key, its words (a link's name in brackets, the words after it), and
    /// whether it's in the monospace type.
    fn shown(rows: Vec<Row>) -> Vec<(&'static str, String)> {
        rows.into_iter()
            .map(|row| match row.value {
                Shown::Text(words, _) => (row.key, words),
                Shown::Id(id) => (row.key, format!("`{id}`")),
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

    fn rows_read(kind: &str, text: &str, read: Option<&SketchRead>) -> Vec<(&'static str, String)> {
        let sketch = |_: &FeatureId| read;
        let context = Context {
            name: &named,
            sketch: &sketch,
        };
        shown(feature_rows(kind, &tree(text), &context))
    }

    fn rows(kind: &str, text: &str) -> Vec<(&'static str, String)> {
        let context = Context {
            name: &named,
            sketch: &no_sketch,
        };
        shown(feature_rows(kind, &tree(text), &context))
    }

    /// The sketch `sketch` (its JSON after `"name"`) read as solved on `plane`, in `state`,
    /// with `regions` regions.
    fn sketch_read(
        sketch: &str,
        plane: SketchPlane,
        state: SketchState,
        regions: usize,
    ) -> SketchRead {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 6, "features": [{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", {sketch}}}]}}"#
        );
        let document = silico_core::document::read(text.as_bytes()).unwrap();
        let Operation::Sketch(members) = document.features[0].operation.clone() else {
            panic!("not a sketch");
        };
        SketchRead {
            sketch: FeatureId::new("s0000001").unwrap(),
            plane: Ok(plane),
            members,
            states: SketchStates {
                state,
                dofs: 2,
                points: Default::default(),
                lines: Default::default(),
                circles: Default::default(),
                arcs: Default::default(),
                constraints: Default::default(),
                solve_problem: None,
            },
            regions: (0..regions)
                .map(|_| silico_core::command::sketch::Region {
                    outer: Vec::new(),
                    holes: Vec::new(),
                })
                .collect(),
            profile_problems: Vec::new(),
            profile_problems_total: 0,
        }
    }

    /// A line from (0, 0) to (40, 0), a construction line, and two arcs about (0, 10), one
    /// of them construction; its dimensions: a length of 40, a reference length, a radius
    /// of 10, a diameter of 11, a distance along x of −12.5, and two angles.
    const DIMENSIONED: &str = r#""plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 40, "y": 0, "construction": false}, {"id": "pa000003", "x": 0, "y": 10, "construction": false}, {"id": "pa000004", "x": 10, "y": 10, "construction": false}, {"id": "pa000005", "x": 0, "y": 20, "construction": false}, {"id": "pa000006", "x": -10, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000001", "to": "pa000003", "construction": true}], "circles": [], "arcs": [{"id": "ab000001", "center": "pa000003", "start": "pa000004", "end": "pa000005", "construction": false}, {"id": "ab000002", "center": "pa000003", "start": "pa000005", "end": "pa000006", "construction": false}, {"id": "ab000003", "center": "pa000003", "start": "pa000006", "end": "pa000001", "construction": true}], "constraints": [{"id": "ka000001", "type": "length", "line": "na000001", "value": 40, "driving": true}, {"id": "ka000002", "type": "length", "line": "na000002", "value": 10, "driving": false}, {"id": "ka000003", "type": "radius", "curve": "ab000001", "value": 10, "driving": true}, {"id": "ka000004", "type": "diameter", "curve": "ab000002", "value": 11, "driving": true}, {"id": "ka000005", "type": "distance_x", "a": "pa000002", "b": "pa000006", "value": -12.5, "driving": true}, {"id": "ka000006", "type": "angle", "a": "na000001", "b": "x_axis", "value": 0.5, "driving": true}, {"id": "ka000007", "type": "angle", "a": "na000002", "b": "x_axis", "value": 90, "driving": true}]"#;

    const CIRCLES: &str = r#""plane": "xz", "points": [{"id": "pc000001", "x": 10, "y": 10, "construction": false}, {"id": "pc000002", "x": 30, "y": 10, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pc000001", "radius": 2.75, "construction": false}, {"id": "ca000002", "center": "pc000002", "radius": 2.75, "construction": false}], "arcs": [], "constraints": []"#;

    fn sketch_shown(read: &SketchRead) -> Vec<(&'static str, String, Color32)> {
        sketch_rows(Some(read))
            .into_iter()
            .map(|row| match row.value {
                Shown::Text(words, colour) => (row.key, words, colour),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_sketchs_rows_are_its_plane_its_definition_its_curves_and_its_dimensions() {
        let read = sketch_read(DIMENSIONED, SketchPlane::Xy, SketchState::FullyDefined, 0);
        assert_eq!(
            sketch_shown(&read),
            [
                ("Plane", "XY".to_owned(), theme::PROPERTY_VALUE),
                // Fully defined in the value's own Gray.800, as `featureProps` leaves it.
                (
                    "Definition",
                    "Fully defined".to_owned(),
                    theme::PROPERTY_VALUE
                ),
                // Construction left out: one line of two, two arcs of three.
                ("Curves", "1 line, 2 arcs".to_owned(), theme::PROPERTY_VALUE),
                // The driving lengths in the order held, the radius and diameter marked as the
                // canvas marks them, the signed distance with its sign, then the angles; the
                // reference length left out.
                (
                    "Dimensions",
                    "40, R10, Ø11, -12.5 mm · 0.5°, 90°".to_owned(),
                    theme::PROPERTY_VALUE
                ),
            ]
        );
        // Circles of one diameter: the prototype's `2 circles, ⌀ 5.5 mm`, on XZ; no
        // dimensions.
        let circles = sketch_read(CIRCLES, SketchPlane::Xz, SketchState::FullyDefined, 2);
        let shown = sketch_shown(&circles);
        assert_eq!(shown[0].1, "XZ");
        assert_eq!(shown[2].1, "2 circles, ⌀ 5.5 mm");
        assert_eq!(shown[3].1, "None");
        // Two diameters: none said.
        let two = CIRCLES.replacen("2.75", "3", 1);
        let two = sketch_read(&two, SketchPlane::Xz, SketchState::FullyDefined, 2);
        assert_eq!(sketch_shown(&two)[2].1, "2 circles");
        // Angles alone.
        let angled = DIMENSIONED
            .replace(
                r#""driving": true}, {"id": "ka000002""#,
                r#""driving": false}, {"id": "ka000002""#,
            )
            .replace(
                r#""value": 10, "driving": true}"#,
                r#""value": 10, "driving": false}"#,
            )
            .replace(
                r#""value": 11, "driving": true}"#,
                r#""value": 11, "driving": false}"#,
            )
            .replace(
                r#""value": -12.5, "driving": true}"#,
                r#""value": -12.5, "driving": false}"#,
            );
        let angled = sketch_read(&angled, SketchPlane::Xy, SketchState::FullyDefined, 0);
        assert_eq!(sketch_shown(&angled)[3].1, "0.5°, 90°");
        // None before it's read.
        assert!(sketch_rows(None).is_empty());
    }

    #[test]
    fn a_sketchs_definition_is_sketch_modes_words_and_colours() {
        // sketch/mod.rs's `state_shown`: under defined with its degrees of freedom left, in
        // `--sk-under`; over defined and no solution in the over-defined red; invalid in its
        // yellow; and an empty sketch Empty, subdued.
        for (state, words, colour) in [
            (
                SketchState::UnderDefined,
                "Under defined · 2 DOF",
                crate::sketch::style::SKETCH_UNDER_DEFINED,
            ),
            (
                SketchState::OverDefined,
                "Over defined",
                crate::sketch::style::SKETCH_OVER_DEFINED,
            ),
            (
                SketchState::NoSolution,
                "No solution found",
                crate::sketch::style::SKETCH_OVER_DEFINED,
            ),
            (
                SketchState::Invalid,
                "Invalid solution found",
                crate::sketch::style::SKETCH_INVALID,
            ),
        ] {
            let read = sketch_read(DIMENSIONED, SketchPlane::Xy, state, 0);
            assert_eq!(
                sketch_shown(&read)[1],
                ("Definition", words.to_owned(), colour),
                "{state:?}"
            );
        }
        let empty = r#""plane": "yz", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []"#;
        let empty = sketch_read(empty, SketchPlane::Yz, SketchState::FullyDefined, 0);
        assert_eq!(
            sketch_shown(&empty),
            [
                ("Plane", "YZ".to_owned(), theme::PROPERTY_VALUE),
                (
                    "Definition",
                    "Empty".to_owned(),
                    crate::sketch::style::TEXT_SUBDUED
                ),
                ("Curves", "None".to_owned(), theme::PROPERTY_VALUE),
                ("Dimensions", "None".to_owned(), theme::PROPERTY_VALUE),
            ]
        );
        // A sketch on a face: `Face`, as the browser's meta.
        let on_face = DIMENSIONED.replace(
            r#""plane": "xy""#,
            r#""plane": {"face": "b0000001.box.Face6", "x": "+x", "reversed": false}"#,
        );
        let frame = silico_protocol::FaceFrame {
            origin: [0.0, 0.0, 10.0],
            x: [1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        };
        let on_face = sketch_read(
            &on_face,
            SketchPlane::Face(frame),
            SketchState::FullyDefined,
            0,
        );
        assert_eq!(sketch_shown(&on_face)[0].1, "Face");
    }

    const PAD: &str = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 12, "reversed": false}}"#;

    #[test]
    fn a_pads_rows_are_its_profile_its_extent_and_its_direction() {
        // app.js's `featureProps`: Profile, Extent (`Distance · 12 mm`), Direction.
        assert_eq!(
            rows("pad", PAD),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Extent", "Distance · 12 mm".to_owned()),
                ("Direction", "Normal".to_owned()),
            ]
        );
        assert_eq!(
            rows("pad", &PAD.replace("false", "true"))[2],
            ("Direction", "Reversed".to_owned())
        );
        // Its sketch read, on XZ: its regions, and the way as the core's rule gives it, along
        // the normal −Y, or against it reversed (the prototype's `Normal (−Y)`).
        let read = sketch_read(CIRCLES, SketchPlane::Xz, SketchState::FullyDefined, 2);
        assert_eq!(
            rows_read("pad", PAD, Some(&read)),
            [
                ("Profile", "[Bracket profile] · 2 regions".to_owned()),
                ("Extent", "Distance · 12 mm".to_owned()),
                ("Direction", "Normal (\u{2212}Y)".to_owned()),
            ]
        );
        let reversed = PAD.replace("false", "true");
        assert_eq!(
            rows_read("pad", &reversed, Some(&read))[2],
            ("Direction", "Reversed (+Y)".to_owned())
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
        // A pad on a base shows it.
        let based = PAD.replace(r#""sketch""#, r#""base": "b0000001", "sketch""#);
        assert_eq!(rows("pad", &based)[3], ("Base", "Upright".to_owned()));
    }

    #[test]
    fn an_extent_the_core_refuses_has_no_row() {
        // A pad can't go through all (`read_extent` refuses it), nor a length be a string:
        // no Extent, and so no Direction.
        let through = PAD.replace(
            r#"{"type": "one_side", "length": 12, "reversed": false}"#,
            r#"{"type": "through_all", "reversed": false}"#,
        );
        assert_eq!(
            rows("pad", &through),
            [("Profile", "[Bracket profile]".to_owned())]
        );
        let text = PAD.replace(r#""length": 12"#, r#""length": "12""#);
        assert_eq!(
            rows("pad", &text),
            [("Profile", "[Bracket profile]".to_owned())]
        );
    }

    #[test]
    fn a_pockets_rows_are_its_profile_its_extent_its_direction_and_its_base() {
        let pocket = r#"{"id": "q0000001", "type": "pocket", "v": 1, "name": "Holes", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "through_all", "reversed": false}}"#;
        assert_eq!(
            rows("pocket", pocket),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Extent", "Through all".to_owned()),
                ("Direction", "Normal".to_owned()),
                ("Base", "Upright".to_owned()),
            ]
        );
        // A pocket cuts against its sketch's normal: on XZ (−Y), +Y; reversed, −Y.
        let read = sketch_read(CIRCLES, SketchPlane::Xz, SketchState::FullyDefined, 2);
        assert_eq!(
            rows_read("pocket", pocket, Some(&read))[2],
            ("Direction", "Normal (+Y)".to_owned())
        );
        assert_eq!(
            rows_read("pocket", &pocket.replace("false", "true"), Some(&read))[2],
            ("Direction", "Reversed (\u{2212}Y)".to_owned())
        );
    }

    #[test]
    fn a_reversed_pocket_reads_otherwise_than_one_that_isnt() {
        let pocket = r#"{"id": "q0000001", "type": "pocket", "v": 1, "name": "H", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
        assert_ne!(
            rows("pocket", pocket),
            rows("pocket", &pocket.replace("false", "true"))
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
    fn a_turns_rows_name_its_axis_its_angle_and_its_sense() {
        let revolve = r#"{"id": "r0000001", "type": "revolve", "v": 1, "name": "Turn", "sketch": "s0000001", "axis": "y_axis", "extent": {"type": "one_side", "angle": 270, "reversed": true}}"#;
        assert_eq!(
            rows("revolve", revolve),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Axis", "Y axis".to_owned()),
                ("Extent", "Angle · 270°".to_owned()),
                // The core's words for a turn's sense: left-handed when reversed.
                ("Direction", "Left-handed".to_owned()),
            ]
        );
        assert_eq!(
            rows("revolve", &revolve.replace("true", "false"))[3],
            ("Direction", "Right-handed".to_owned())
        );
        // A line of the sketch as the axis: its ID, in the monospace type.
        let groove = r#"{"id": "g0000001", "type": "groove", "v": 1, "name": "Ring", "base": "b0000001", "sketch": "s0000001", "axis": "na000001", "extent": {"type": "symmetric", "angle": 90}}"#;
        assert_eq!(
            rows("groove", groove),
            [
                ("Profile", "[Bracket profile]".to_owned()),
                ("Axis", "`na000001`".to_owned()),
                ("Extent", "Symmetric · 90°".to_owned()),
                ("Base", "Upright".to_owned()),
            ]
        );
    }

    #[test]
    fn a_booleans_rows_are_its_target_and_its_tool() {
        let cut = r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "b0000001", "tool": "t0000001"}"#;
        for kind in ["fuse", "cut", "common"] {
            assert_eq!(
                rows(kind, &cut.replace("\"cut\"", &format!("\"{kind}\""))),
                [
                    ("Target Body", "Upright".to_owned()),
                    ("Tool Body", "Pin".to_owned()),
                ]
            );
        }
        // A primitive has none.
        let block = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
        assert_eq!(rows("box", block), []);
    }

    /// Polls `read` until `done` holds of it, failing after a minute.
    fn polled_until(
        read: &mut FeatureRead,
        sessions: &Sessions<std::sync::Arc<silico_core::fake::FakeKernel>>,
        session: &SessionId,
        revision: Revision,
        done: impl Fn(&FeatureRead) -> bool,
    ) {
        let feature = FeatureId::new("k7f2qx3m").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !done(read) {
            assert!(std::time::Instant::now() < deadline, "never done");
            read.poll(sessions, session, &feature, revision);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_read_refused_once_it_ran_keeps_the_last_text_and_one_that_never_ran_is_sent_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part.silico");
        std::fs::write(
            &path,
            r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 6, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}]}"#,
        )
        .unwrap();
        let kernel = std::sync::Arc::new(silico_core::fake::FakeKernel::new(1).unwrap());
        let sessions =
            Sessions::with_cache_opener(kernel, |dir| silico_core::cache::Cache::unsealed(dir));
        let opened = sessions.open(&path, false).wait();
        let session = opened.session.unwrap().id;
        let block = FeatureId::new("k7f2qx3m").unwrap();
        let mut read = FeatureRead::default();
        // The opened revision's text read once, and held.
        polled_until(&mut read, &sessions, &session, Revision::OPENED, |read| {
            read.node(&block).is_some()
        });
        assert_eq!(read.reads, 1);
        // A revision the session never held: refused once it ran. The block's text is kept,
        // and the read isn't sent again for that revision.
        let unheld = Revision::OPENED.next().unwrap().next().unwrap();
        polled_until(&mut read, &sessions, &session, unheld, |read| {
            read.reads == 2 && read.reading.is_none()
        });
        for _ in 0..20 {
            read.poll(&sessions, &session, &block, unheld);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(read.reads, 2);
        assert!(read.node(&block).is_some(), "the last text kept");
        // A session not open: refused before it ran, so sent again by a later poll.
        let gone = SessionId::parse("s999").unwrap();
        polled_until(&mut read, &sessions, &gone, Revision::OPENED, |read| {
            read.reads >= 5
        });
        assert!(read.node(&block).is_some(), "the last text kept");
    }

    #[test]
    fn every_kind_but_the_primitives_has_parameters() {
        // The format's every type (`FEATURE_TYPES`), and by hand the primitives, which the
        // app makes none of.
        let primitives = ["box", "cylinder", "sphere", "cone", "torus"];
        for kind in silico_core::document::FEATURE_TYPES {
            assert_eq!(has_parameters(kind), !primitives.contains(&kind), "{kind}");
        }
    }

    #[test]
    fn a_ways_axis_is_signed_with_a_true_minus_and_an_oblique_one_written_whole() {
        let one_side = |reversed| {
            Extent::Pad(PadExtent::OneSide {
                length: Length::parse("1").unwrap(),
                reversed,
            })
        };
        let words = |extent, plane: SketchPlane| {
            direction_words(SweepFeature::Pad, extent, Some(plane.normal())).unwrap()
        };
        // SketchPlane's normals: XY's +Z, XZ's −Y, YZ's +X.
        assert_eq!(
            words(one_side(false), SketchPlane::Xz),
            "Normal (\u{2212}Y)"
        );
        assert_eq!(words(one_side(true), SketchPlane::Xz), "Reversed (+Y)");
        assert_eq!(words(one_side(false), SketchPlane::Xy), "Normal (+Z)");
        assert_eq!(
            words(one_side(true), SketchPlane::Yz),
            "Reversed (\u{2212}X)"
        );
        // An oblique face's normal, reversed: each component negated, its zero written 0,
        // never -0.
        let oblique = [0.6, 0.0, 0.8];
        assert_eq!(
            direction_words(SweepFeature::Pad, one_side(true), Some(oblique)).unwrap(),
            "Reversed (-0.6, 0, -0.8)"
        );
    }
}
