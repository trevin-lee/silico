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
        | Command::Text
        | Command::Outline
        | Command::Check
        | Command::Elements
        | Command::Query
        | Command::Schema
        | Command::MassProperties
        | Command::Distance
        | Command::Angle => Effect::Reads,
        // An edit outside a transaction saves at once; commit and format save the file.
        Command::Commit
        | Command::Format
        | Command::AddFeature
        | Command::SetField
        | Command::RemoveFeature
        | Command::ReplaceText => Effect::Changes { destructive: true },
        // `replace: true` writes over a file at `to`.
        Command::Render => Effect::Changes { destructive: true },
        // A new document is only made where nothing is; the rest change what the server
        // holds, never a file.
        Command::Open | Command::Close | Command::Cancel | Command::Begin | Command::Rollback => {
            Effect::Changes { destructive: false }
        }
    }
}

/// The three things an agent most needs (M4.1 section 9), said in `initialize`'s
/// instructions and in the descriptions of the tools they're about.
pub const IN_A_TRANSACTION: &str = "Edit inside a transaction: `begin`, the edits, then `commit` to save them or `rollback` to undo them all. An edit made outside one is saved to the file at once, with no undo but another edit.";
pub const READ_CURRENT: &str = "After an edit, read with `model: \"current\"`, which waits for the edit's rebuild; `published` answers at once from the newest model rebuilt, which may be older.";
pub const NAMES_FROM_ELEMENTS: &str =
    "Name a face, edge, or vertex only as `elements` or `query` gives it; never make a name up.";

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
            "What a session holds, answered at once even while another of its commands runs: its features, whether a transaction is open, what's running, and how far its rebuild has got.",
        ),
        Command::Cancel => (
            "Cancel a command",
            "Stop the command named, if it's running in the session. The command stopped answers too: `cancelled` if it left no trace, or its real result if it had already done its work.",
        ),
        Command::Begin => (
            "Begin a transaction",
            "Open a transaction in the session: until `commit` or `rollback`, edits change only what the session holds, never the file.",
        ),
        Command::Commit => (
            "Commit",
            "End the session's transaction, saving its document with the fingerprints of what its edits reached. Refused as `conflict` if the file changed outside the session since: roll back, or begin again from the file.",
        ),
        Command::Rollback => (
            "Roll back",
            "End the session's transaction, undoing every edit made in it. The file is never touched.",
        ),
        Command::Format => (
            "Format",
            "Save the document in canonical form with every feature's fingerprint as rebuilt, as `silico fmt` does, accepting the drift of the features named. Refused inside a transaction.",
        ),
        Command::AddFeature => (
            "Add a feature",
            "Add a feature at the end of the session's timeline. Give the revision the edit was made against, as the session's last answer says it.",
        ),
        Command::SetField => (
            "Set a field",
            "Set one field of one feature, by the feature's ID and the field's path (`extent.length`; a sketch's members by their IDs). Give the revision the edit was made against.",
        ),
        Command::RemoveFeature => (
            "Remove a feature",
            "Remove one feature, which no other may use; its JSON is returned. Give the revision the edit was made against.",
        ),
        Command::ReplaceText => (
            "Replace the text",
            "Replace the document's whole text, read and checked as `silico check` reads a file; the text replaced is returned. Give the revision the edit was made against.",
        ),
        Command::Text => (
            "Document text",
            "The document's text as the session holds it, or one feature's.",
        ),
        Command::Outline => (
            "Outline",
            "The document's outline, one line a feature with its values, as `silico show` prints it.",
        ),
        Command::Check => (
            "Check",
            "The document's problems and warnings, from the model named: failed features, broken references, drift.",
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
        | Command::ReplaceText => &[IN_A_TRANSACTION, READ_CURRENT],
        Command::Check | Command::MassProperties | Command::Render => &[READ_CURRENT],
        Command::Elements | Command::Query | Command::Distance | Command::Angle => {
            &[READ_CURRENT, NAMES_FROM_ELEMENTS]
        }
        Command::Version
        | Command::Open
        | Command::Close
        | Command::Status
        | Command::Cancel
        | Command::Rollback
        | Command::Format
        | Command::Text
        | Command::Outline
        | Command::Schema => &[],
    };
    let mut description = what.to_owned();
    for tip in tips {
        description.push(' ');
        description.push_str(tip);
    }
    (title, description)
}
