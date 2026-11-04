//! The tool list (docs/design/m4.9-mcp.md, *Tools*): one tool per command, in
//! `wire::COMMANDS`'s order, each with the command's name, a title, a description written
//! for agents, MCP's annotations, its request's snapshot as its `inputSchema`, and an
//! `outputSchema` built from the envelope's snapshot and its result's ([`output_schema`]),
//! since MCP checks `structuredContent`, the whole envelope, against it.

use serde_json::{Map, Value, json};
use silico_core::command::Command;
use silico_core::command::wire::{self, COMMANDS};
use silico_core::worker::SILICO_BUG;
use silico_core::{Error, ErrorCode};

/// The JSON Schema dialect of every schema here, as the core's snapshots are.
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// `tools/list`'s result, `{"tools": [...]}`, from the core's snapshots: every tool, in
/// `COMMANDS`' order, each object's keys sorted.
pub fn list() -> Result<Value, Error> {
    let envelope = wire::snapshot_value("envelope")
        .ok_or_else(|| bug("the core has no envelope snapshot".to_owned()))?;
    let mut tools = Vec::with_capacity(COMMANDS.len());
    for (name, snapshot) in COMMANDS {
        let command =
            Command::named(name).ok_or_else(|| bug(format!("{name} is listed, not a command")))?;
        tools.push(tool(command, &envelope, &snapshot())?);
    }
    Ok(silico_core::document::sorted(&json!({ "tools": tools })))
}

/// The tool for `command`, from the envelope's snapshot and the command's own
/// (`{"request": ..., "result": ...}`).
pub fn tool(command: Command, envelope: &Value, snapshot: &Value) -> Result<Value, Error> {
    let name = command.as_str();
    let request = snapshot
        .get("request")
        .ok_or_else(|| bug(format!("{name}'s snapshot has no request")))?;
    let result = snapshot
        .get("result")
        .ok_or_else(|| bug(format!("{name}'s snapshot has no result")))?;
    let output = output_schema(envelope, result).map_err(|e| {
        Error::new(
            e.code(),
            format!("{name}'s output schema couldn't be built: {}", e.message()),
        )
        .with_suggestion(SILICO_BUG)
    })?;
    let (title, description) = words(command);
    let mut annotations = Map::new();
    annotations.insert("title".to_owned(), json!(title));
    match effect(command) {
        Effect::Reads => {
            annotations.insert("readOnlyHint".to_owned(), json!(true));
        }
        Effect::Changes { destructive } => {
            annotations.insert("readOnlyHint".to_owned(), json!(false));
            annotations.insert("destructiveHint".to_owned(), json!(destructive));
        }
    }
    annotations.insert("openWorldHint".to_owned(), json!(false));
    Ok(json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": request,
        "outputSchema": output,
        "annotations": annotations,
    }))
}

/// The `outputSchema` of a command whose result's snapshot is `result`: a root of type
/// object (as MCP asks), `{"$schema", "type": "object", "anyOf": [ran, refused], "$defs"}`,
/// its branches the envelope's (`envelope`, its snapshot), the one that ran with its
/// `result` the command's result schema, inlined without its `$schema` and `title` (2020-12
/// allows `$schema` only at a resource's root), its `$defs` lifted to the root, where its
/// `#/$defs/...` references resolve. The envelope's `Ran` isn't kept: its `result` is any
/// value. A name in both `$defs` must define the same schema in both: one that differs is
/// refused, never silently picked.
pub fn output_schema(envelope: &Value, result: &Value) -> Result<Value, Error> {
    let mut defs = envelope
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| bug("the envelope's schema has no $defs".to_owned()))?;
    let mut ran = defs
        .remove("Ran")
        .ok_or_else(|| bug("the envelope's schema defines no Ran".to_owned()))?;
    if !defs.contains_key("Refused") {
        return Err(bug("the envelope's schema defines no Refused".to_owned()));
    }
    let mut result = result
        .as_object()
        .cloned()
        .ok_or_else(|| bug("the result's schema isn't an object".to_owned()))?;
    result.remove("$schema");
    result.remove("title");
    let lifted = match result.remove("$defs") {
        None => Map::new(),
        Some(Value::Object(lifted)) => lifted,
        Some(_) => return Err(bug("the result's $defs isn't an object".to_owned())),
    };
    for (name, def) in lifted {
        match defs.get(&name) {
            Some(same) if *same == def => {}
            Some(_) => {
                return Err(bug(format!(
                    "the result's definition {name} differs from the envelope's"
                )));
            }
            None => {
                defs.insert(name, def);
            }
        }
    }
    let slot = ran
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("result"))
        .ok_or_else(|| bug("the envelope's Ran has no result".to_owned()))?;
    *slot = Value::Object(result);
    Ok(json!({
        "$schema": DIALECT,
        "type": "object",
        "anyOf": [ran, {"$ref": "#/$defs/Refused"}],
        "$defs": defs,
    }))
}

/// `value` written as the protected contract files are, by the core's writer of them:
/// every object's keys sorted, pretty, with a trailing newline.
pub fn canonical(value: &Value) -> Result<String, Error> {
    silico_core::document::contract_text(value, "the tool list")
}

fn bug(message: String) -> Error {
    Error::new(ErrorCode::Internal, message).with_suggestion(SILICO_BUG)
}

/// What a tool does to what's outside the server, as MCP's annotations say it.
enum Effect {
    /// It reads only (`readOnlyHint`).
    Reads,
    /// It changes something: a file, or what a session holds; `destructive` if it may
    /// write over what was there (`destructiveHint`), not only add.
    Changes { destructive: bool },
}

/// What `command` does, every command matched, so a new one must say.
fn effect(command: Command) -> Effect {
    match command {
        Command::Version
        | Command::Status
        | Command::ReadLogs
        | Command::Text
        | Command::Outline
        | Command::Diff
        | Command::Check
        | Command::Elements
        | Command::Query
        | Command::Schema
        | Command::MassProperties
        | Command::Distance
        | Command::Angle
        | Command::Timeline
        | Command::Meshes
        | Command::Sketch => Effect::Reads,
        // An edit outside a transaction saves at once; commit and format save the file.
        Command::Commit
        | Command::Format
        | Command::AddFeature
        | Command::SetField
        | Command::RemoveFeature
        | Command::ReplaceText
        | Command::MoveFeature
        | Command::SuppressFeature
        | Command::DragEnd
        | Command::SketchAdd
        | Command::SketchRemove
        | Command::SketchSplit
        | Command::SketchTrim
        | Command::SketchUnjoin
        | Command::SketchMerge
        | Command::SketchExtend
        | Command::SketchJoin => Effect::Changes { destructive: true },
        // A drag is held by the server, and its moves change no file.
        Command::DragBegin | Command::DragMove => Effect::Changes { destructive: false },
        // `replace: true` writes over a file at `to`, or at `path`.
        Command::Render | Command::Export => Effect::Changes { destructive: true },
        // A new document is only made where nothing is; the rest change what the server
        // holds, never a file. An undo or a redo can drop the stack's oldest states past
        // its bound, but it never touches a file, and `rollback` still reaches `begin`'s.
        Command::Open
        | Command::Close
        | Command::Cancel
        | Command::Begin
        | Command::Rollback
        | Command::Undo
        | Command::Redo => Effect::Changes { destructive: false },
        // Removes only the cache's own entries, each a miss the next rebuild makes again.
        Command::CacheClear => Effect::Changes { destructive: false },
    }
}

/// The three things an agent most needs (M4.1 section 9), said in `initialize`'s
/// instructions and in the descriptions of the tools they're about.
pub const IN_A_TRANSACTION: &str = "Edit inside a transaction: `begin`, the edits, then `commit` to save them or `rollback` to undo them all. An edit made outside one is saved to the file at once, with no undo but another edit.";
pub const READ_CURRENT: &str = "After an edit, read with `model: \"current\"`, which waits for the edit's rebuild; `published` answers at once from the newest model rebuilt, which may be older.";
pub const NAMES_FROM_ELEMENTS: &str =
    "Name a face, edge, or vertex only as `elements` or `query` gives it; never make a name up.";
/// That a part can be made with no file (an untitled one), said in `initialize`'s
/// instructions and in `open`'s description, as the three above are in theirs.
pub const UNTITLED: &str = "For a part with no file yet, open with `path: null` and `create: true`: an untitled part, nothing on disk, its edits made inside a transaction, and its first `commit` given a path to save it there.";

/// `command`'s title and description, for an agent: every command matched.
fn words(command: Command) -> (&'static str, String) {
    let (title, what) = match command {
        Command::Version => (
            "Version",
            "The API's version, the document format's and naming scheme's, and the limits every command holds to (request and response sizes, sessions open, list lengths, time limits). Takes no session.",
        ),
        Command::Open => (
            "Open a document",
            "Open a document's file as a session, or make a new, empty one (`create: true`, refused if anything is at the path). The path is relative to the workspace and may not lead out of it. Give the session's ID (`s1`, ...) in every later request to it. A document the gate refuses still opens, its problems in the result, so it can be mended with `replace_text`.",
        ),
        Command::Close => (
            "Close a session",
            "Close a session. Refused while a transaction is open: commit or roll back first.",
        ),
        Command::Status => (
            "Session status",
            "What a session holds, answered at once even while another of its commands runs: its features; the transaction open, if one is, with whether its document differs from the one it began from (unsaved), and the edits `undo` and `redo` can step through; what's running; and how far its rebuild has got.",
        ),
        Command::ReadLogs => (
            "Read logs",
            "A session's recent log events, answered at once even while another of its commands runs: those of a level and every level more severe, of one feature or every feature, after a cursor (the last read's `next`, or null from the oldest kept). Diagnostics: an event's time and its order among other threads' depend on the machine, never on the model.",
        ),
        Command::Cancel => (
            "Cancel a command",
            "Stop the command named, if it's running in the session, or, named `rebuild`, the session's rebuild while `status` says it's running, with the command waiting on it. The command stopped answers too: `cancelled` if it left no trace, or its real result if it had already done its work.",
        ),
        Command::Begin => (
            "Begin a transaction",
            "Open a transaction in the session: until `commit` or `rollback`, edits change only what the session holds, never the file.",
        ),
        Command::Commit => (
            "Commit",
            "End the session's transaction, saving its document with the fingerprints of what its edits reached. Refused as `conflict` if the file changed outside the session since: roll back, or begin again from the file, or save elsewhere with `path`. With `path`, saves to that file instead (Save As), which the session then holds.",
        ),
        Command::Rollback => (
            "Roll back",
            "End the session's transaction, undoing every edit made in it. The file is never touched.",
        ),
        Command::Undo => (
            "Undo",
            "Inside a transaction, undo its last edit not undone, or with `to` (an earlier revision the session was at in it), every edit after that revision: its state restored byte for byte, at a new revision. Each edit is one step: to undo a run of edits as one, undo `to` the revision the run began at. The result says the edits left to undo and redo, and `dropped`, how many more edits the undo stack's bound (`version`'s `undo_bytes`) put out of reach; `rollback` still returns to the transaction's start. Outside a transaction each edit was saved as it was made, so there's nothing to undo. Give the revision the session's last answer says.",
        ),
        Command::Redo => (
            "Redo",
            "Inside a transaction, redo the last edit undone, or with `to` (a later revision the session was at in it), every edit undone up to that revision: its state restored byte for byte, at a new revision. An edit after an undo drops what was undone, which can't be redone. Give the revision the session's last answer says.",
        ),
        Command::Format => (
            "Format",
            "Save the document in canonical form with every feature's fingerprint as rebuilt, as `silico fmt` does, accepting the drift of the features named. Refused inside a transaction.",
        ),
        Command::AddFeature => (
            "Add a feature",
            "Add a feature at the end of the session's timeline (`before: null`), or before another: then each input it takes (its base, its tool) that a feature after it consumed is consumed by it instead, that feature's field rewired to it, listed in `rewired`. A sketch on a face may leave out its plane's `x`: the core writes the face's `default_x`, as `elements` reports it, waiting for the model's rebuild. Give the revision the edit was made against, as the session's last answer says it.",
        ),
        Command::SetField => (
            "Set a field",
            "Set one field of one feature, by the feature's ID and the field's path (`extent.length`, a sketch's `plane` or its face plane's `plane.face`; a sketch's members by their IDs). Give the revision the edit was made against.",
        ),
        Command::RemoveFeature => (
            "Remove a feature",
            "Remove one feature; its JSON is returned. The feature that consumed it takes its base, listed in `rewired`. Refused, naming both, where nothing can stand in: a chain's first feature under one that needs a base, a feature's sketch, or a feature whose elements another names. To put it back, give `add_feature` the returned feature without its `fingerprint`, `before` the feature that followed it (inside a transaction, the fingerprint `begin` saw is kept), then `set_field` `base` back on each `rewired` entry whose `to` is null; or remove inside a transaction and `rollback`. Give the revision the edit was made against.",
        ),
        Command::MoveFeature => (
            "Move a feature",
            "Move one feature before another, or to the end (`before: null`), rewiring the chains it leaves and joins: its consumer takes its base, it builds on the last feature of its chain before the new place, and the next one builds on it, each change listed in `rewired`. Refused, naming both, where a child would come before its parent (its sketch, its tool, a feature whose elements it names). Give the revision the edit was made against.",
        ),
        Command::SuppressFeature => (
            "Suppress a feature",
            "Suppress one feature (`suppressed: true`) or unsuppress it (`false`). A suppressed feature isn't built: a feature that consumes it builds on the nearest unsuppressed feature up its chain, so suppressing a part's first hole leaves the part. Its dependents go with it: a feature whose sketch it is, one naming its elements, and one left with an input it needs and nothing unsuppressed up its chain. Unsuppressing a feature unsuppresses what it needs (its sketch, the features its names come from, its chain's first feature if every one is suppressed); what depends on it stays suppressed. Each list is in `suppressed` and `unsuppressed`, in timeline order. Give the revision the edit was made against.",
        ),
        Command::ReplaceText => (
            "Replace the text",
            "Replace the document's whole text, read and checked as `silico check` reads a file; the text replaced is returned. Give the revision the edit was made against.",
        ),
        Command::SketchAdd => (
            "Add to a sketch",
            "Add points, lines, circles, arcs, and constraints to one sketch, each as the document writes it with a fresh random 8-character ID, never one the sketch holds or a removal freed (a name a feature stores of the removed member would find the new one); a new curve may end at a point the sketch holds, joining it there. The batch is checked whole, each problem at its member's path (`circles.cb000003.radius`). A constraint that over-defines the sketch is added all the same: `check` then reports the conflict, and the sketch's features fail until it's mended (make the dimension driven with `set_field`, or remove it). Give the revision the edit was made against.",
        ),
        Command::SketchRemove => (
            "Remove from a sketch",
            "Remove members of one sketch by ID: a constraint; a curve, with its points no other curve uses and every constraint on what goes; a point only if no curve uses it. A sketch left under or over defined, or unsolvable, isn't refused: `check` reports it. Give the revision the edit was made against.",
        ),
        Command::DragBegin => (
            "Begin a drag",
            "Begin dragging one sketch's point (`point`), or a curve's edge (`curve`, and `press`, where on it the press landed, in the sketch's coordinates), the other null: a line's edge moves the line whole, a circle's or an arc's sizes it through the cursor. Every point dragged is pinned below every constraint, so dimensions hold. The result lists the points of the part that moves. One drag is open in a session at once; it ends at `drag_end`, at any edit, undo, or reload of the session, or if its worker fails. A sketch whose constraints conflict can't be dragged. An untitled session (opened with `path: null`) drags only inside a transaction: outside one, `drag_end` would save an edit it has no file for, so `drag_begin` is refused.",
        ),
        Command::DragMove => (
            "Move a drag",
            "Move the open drag's cursor to `to`, in the sketch's coordinates, and get the frame: the part's points and free radii where the solve put them (`solved`), or none (`failed`, `invalid`, or `dropped` when a newer move replaced it before it was solved). Send moves only once `drag_begin` has answered. A frame changes nothing: only `drag_end` edits the document.",
        ),
        Command::DragEnd => (
            "End a drag",
            "End the open drag: with `to`, the sketch moved there from where the drag began and solved as a whole, its points saved as one edit (`kept: cursor`); with `to: null`, or if the move or the solve fails, the sketch is left as it began (`kept: begin`), with no edit. Refused, the sketch as it was, if its solve then flags a redundant constraint the solve before the drag didn't: a problem the sketch had already isn't the drag's.",
        ),
        Command::SketchSplit => (
            "Split a sketch curve",
            "Split one line or arc of a sketch at a position on it (within 1e-7 mm), or a circle at two, into pieces sharing a new point; the piece on the curve's start side keeps its ID, and `references` lists the features whose stored names the curve reached, which now find only that piece. Constraints on the curve follow the pieces (a length becomes a distance of the outer ends). `ids` gives the new members' IDs in `added`'s order, or null for the core's. Refused if the sketch's solve is then worse than before it (a new conflict or redundancy, or no solution): a conflict the sketch had already isn't the edit's. Give the revision the edit was made against.",
        ),
        Command::SketchTrim => (
            "Trim a sketch curve",
            "Trim one curve of a sketch back to the curves that cut it either side of `pick`, removing that part, or the whole curve where nothing cuts it there. A new end shares a cutter's end held on the curve, or gets a new point held on the cutter. `ids` gives the new members' IDs in `added`'s order, or null for the core's. Refused if the sketch's solve is then worse than before it (a new conflict or redundancy, or no solution): a conflict the sketch had already isn't the edit's. Give the revision the edit was made against.",
        ),
        Command::SketchUnjoin => (
            "Unjoin a sketch point",
            "Pull one curve off a point it shares with others: it gets a new point at the same place, keeping its own constraints; the others keep the point and its constraints. `check` then warns of the two points as `ends_not_joined` until something moves them. `ids` is the new point's ID, a list of one, or null for the core's. Refused if the sketch's solve is then worse than before it (a new conflict or redundancy, or no solution): a conflict the sketch had already isn't the edit's. Give the revision the edit was made against.",
        ),
        Command::SketchExtend => (
            "Extend a sketch curve",
            "Extend one line's or arc's `start` or `end`: `by` a length (a line's, in mm) or an angle (an arc's, in degrees), positive to lengthen; to a point on its line or circle; or to the next curve it meets past the end, which the core finds. A line's end moves through the solver's drag, so its constraints win and curves sharing the end follow; the end is then put exactly where named. Nothing is added to hold it there: to join it to the curve it meets, merge the two points (`sketch_merge`). Refused if the sketch's solve is then worse than before it (a new conflict or redundancy, or no solution): a conflict the sketch had already isn't the edit's, but a line's end moves only in a sketch whose constraints agree; in one that conflicts it's refused `over_constrained`, naming the conflict, as a drag is. Give the revision the edit was made against.",
        ),
        Command::SketchMerge => (
            "Merge sketch points",
            "Make two or more points of a sketch one: the first named survives at its own coordinates, and the others' curves and constraints move to it. Refused if the sketch's solve is then worse than before it (a new conflict or redundancy, or no solution): name first the point the constraints hold. `silico fmt --merge-touching` merges every `ends_not_joined` cluster so. Give the revision the edit was made against.",
        ),
        Command::SketchJoin => (
            "Join a split's pieces",
            "Join the two pieces a split left at a point into one curve again, split's inverse: two lines held on one line by a `tangent_at` (`same`) at the point, or two arcs about one centre (a split circle's two arcs make its circle again). Name the point they share. The piece that ends there keeps its ID and takes the other's far end (it's the one curve in `modified`, the other piece in `removed`); the other piece, the point, and the constraints naming the point go, and constraints on the other piece move to the kept one. Nothing is retyped: a split's distance of the outer ends stays a distance. `references` lists the features whose stored names either piece reached. Refused for a piece's length, midpoint, or equal with another line, which would then measure the whole line: remove it, or dimension the outer ends. Refused if the sketch's solve is then worse than before it (a new conflict or redundancy, or no solution): a conflict the sketch had already isn't the edit's. Give the revision the edit was made against.",
        ),
        Command::Text => (
            "Document text",
            "The document's text as the session holds it, or one feature's.",
        ),
        Command::Outline => (
            "Outline",
            "The document's outline, one line a feature with its values, as `silico show` prints it.",
        ),
        Command::Diff => (
            "Changes between revisions",
            "What changed between two revisions the session holds (`null` for the document now): each feature added, removed, or moved (`field` null, `before` and `after` its place in the timeline), and each field that changed, by the path `set_field` names it by, with its value before and after. The session holds its revision now, the one its last reload replaced, and inside a transaction the one it began at: after a response that says `reloaded`, diff from the revision you last saw to `null` to learn what changed outside the session.",
        ),
        Command::Timeline => (
            "Timeline",
            "The document's features in timeline order, structured: each one's ID, type, name, inputs, and health (healthy, warning, error, suppressed, or unknown) from the model named, with how many problems and warnings it has (`check` gives their text); and the bodies. A document the gate refuses is listed too, every health unknown.",
        ),
        Command::Check => (
            "Check",
            "The document's problems and warnings, from the model named: failed features, broken references, drift; and, failing nothing, ends FreeCAD would join, redundant constraints that agree, and features saved without a fingerprint.",
        ),
        Command::CacheClear => (
            "Clear the cache",
            "Remove every entry of the rebuild cache beside the session's document (`.silico-cache/`), and only the files the cache made. The cache is the directory's, shared by every document in it; an entry removed is only a miss, made again by the next rebuild. To rebuild without the cache once, `check` with `fresh: true` instead.",
        ),
        Command::Elements => (
            "List elements",
            "A page of the model's faces, edges, and vertices, of one feature's solid or of every body, each with its name, its geometry, and its measures.",
        ),
        Command::Query => (
            "Query elements",
            "The model's elements of a kind that match a structured filter, such as the largest planar face facing +Z.",
        ),
        Command::Schema => (
            "Schema",
            "The document's JSON Schema, or a command's request and result schemas. Takes no session.",
        ),
        Command::MassProperties => (
            "Mass properties",
            "The volume, area, and centre of mass of one feature's solid, or of every body.",
        ),
        Command::Distance => (
            "Distance",
            "The least distance between two elements, and their centres', normal, and centre-to-element distances where they have them.",
        ),
        Command::Angle => (
            "Angle",
            "The angle between two elements, each a straight edge or a planar face.",
        ),
        Command::Export => (
            "Export",
            "Write the current model's bodies, or the features named, to `path` in the workspace: as a printable 3MF, each solid meshed at the tolerance (FreeCAD's default is 0.1 mm), or as STEP, exact, with `tolerance: null`. A file already at `path` is replaced only with `replace: true`.",
        ),
        Command::Sketch => (
            "Read a sketch",
            "A sketch as the model solved it: its plane in 3D, its state (under, fully, or over defined, no solution, or invalid) and degrees of freedom, each point, curve, and constraint as the document writes it, at its solved values, with its own state, the regions its profile makes or why it makes none, and why it wasn't solved.",
        ),
        Command::Meshes => (
            "Display meshes",
            "The display meshes of the model's bodies for a view (a named view or a camera, and its size in pixels), as the app's viewport draws them: each body, or each feature named in `features`, at the largest power of two no coarser than half a pixel, from the meshes `render` makes. Each body's positions, normals, and triangles, its edges' polylines, each face and edge by its name, its `key`, and the half pixels its level `serves`. Pass back in `held` the `key` and `deflection` of each mesh already read: a body held is named, but its geometry isn't given again. To look at the model, `render` is cheaper.",
        ),
        Command::Render => (
            "Render",
            "A PNG of the model's bodies from a named view or a camera, with elements highlighted, returned as an image beside the result, and written to `to` in the workspace if given.",
        ),
    };
    let tips: &[&str] = match command {
        Command::Begin
        | Command::Commit
        | Command::AddFeature
        | Command::SetField
        | Command::RemoveFeature
        | Command::ReplaceText
        | Command::MoveFeature
        | Command::SuppressFeature
        | Command::DragEnd
        | Command::SketchAdd
        | Command::SketchRemove
        | Command::SketchSplit
        | Command::SketchTrim
        | Command::SketchUnjoin
        | Command::SketchMerge
        | Command::SketchExtend
        | Command::SketchJoin => &[IN_A_TRANSACTION, READ_CURRENT],
        Command::Check
        | Command::MassProperties
        | Command::Render
        | Command::Timeline
        | Command::Meshes
        | Command::Sketch
        | Command::Undo
        | Command::Redo => &[READ_CURRENT],
        Command::Open => &[UNTITLED],
        Command::Elements | Command::Query | Command::Distance | Command::Angle => {
            &[READ_CURRENT, NAMES_FROM_ELEMENTS]
        }
        Command::Version
        | Command::Close
        | Command::Status
        | Command::ReadLogs
        | Command::Cancel
        | Command::Rollback
        | Command::Format
        | Command::Text
        | Command::Outline
        | Command::Diff
        | Command::Schema
        | Command::CacheClear
        | Command::Export
        | Command::DragBegin
        | Command::DragMove => &[],
    };
    let mut description = what.to_owned();
    for tip in tips {
        description.push(' ');
        description.push_str(tip);
    }
    (title, description)
}
