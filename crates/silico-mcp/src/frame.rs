//! JSON-RPC's frames, as lines (docs/design/m4.9-mcp.md, *Exact JSON both ways*): each line
//! read up to [`MAX_LINE`] bytes, a longer one skipped to its end without being held past
//! that ([`read_line`]); each parsed by serde_json into a frame whose `id` and `params` stay
//! raw text ([`parse`]), so a call's arguments reach the core as the bytes the client sent.

use std::io::{self, BufRead};

use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use silico_core::document::json::MAX_INPUT_LEN;

/// The room a line has around a call's arguments: the frame's own keys, its ID, its method,
/// the tool's name, and a progress token.
pub const FRAME_ROOM: usize = 1 << 20;

/// The most bytes of a line read: the core's [`MAX_INPUT_LEN`], a request's arguments at
/// most, and the frame's room.
pub const MAX_LINE: usize = MAX_INPUT_LEN + FRAME_ROOM;

/// A line read from the client.
pub enum Line {
    /// A line, its newline (and a carriage return before it) taken off.
    Whole(Vec<u8>),
    /// A line longer than the most read: its first `MAX_LINE` bytes, and how long it was.
    TooLong { head: Vec<u8>, length: u64 },
    /// The input ended.
    End,
    /// Reading failed.
    Failed(io::Error),
}

/// The next line of `input`, of at most `max` bytes: a longer one is read to its end, its
/// bytes past `max` dropped as they're read, never held.
pub fn read_line(input: &mut impl BufRead, max: usize) -> Line {
    let mut line = Vec::new();
    let mut length: u64 = 0;
    loop {
        let chunk = match input.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Line::Failed(e),
        };
        if chunk.is_empty() {
            // The input ended: a last line without its newline is a line still.
            if length == 0 {
                return Line::End;
            }
            return ended(line, length, max);
        }
        let (taken, done) = match chunk.iter().position(|&b| b == b'\n') {
            Some(at) => (at, true),
            None => (chunk.len(), false),
        };
        let room = max.saturating_sub(line.len()).min(taken);
        line.extend_from_slice(&chunk[..room]);
        length = length.saturating_add(taken as u64);
        input.consume(if done { taken + 1 } else { taken });
        if done {
            return ended(line, length, max);
        }
    }
}

/// A line read whole, or, past `max`, its head.
fn ended(mut line: Vec<u8>, length: u64, max: usize) -> Line {
    if length > max as u64 {
        return Line::TooLong { head: line, length };
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Line::Whole(line)
}

/// A request's ID: a string or a number, kept as the client wrote it, so its reply carries
/// it back byte for byte. Two IDs are the same if they're the same string (however it's
/// escaped) or the same number as written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Id {
    key: IdKey,
    raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum IdKey {
    Number(String),
    Text(String),
}

impl Id {
    /// The ID `raw` is, if it's a string or a number.
    pub fn of(raw: &RawValue) -> Option<Id> {
        let text = raw.get();
        let key = match text.as_bytes().first()? {
            b'"' => IdKey::Text(serde_json::from_str::<String>(text).ok()?),
            b'-' | b'0'..=b'9' => IdKey::Number(text.to_owned()),
            _ => return None,
        };
        Some(Id {
            key,
            raw: text.to_owned(),
        })
    }

    /// The ID as the client wrote it: JSON text.
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

/// A message from the client.
pub enum Message<'a> {
    /// A request, which is answered.
    Request {
        id: Id,
        method: String,
        params: Option<&'a RawValue>,
    },
    /// A notification, which never is.
    Notification {
        method: String,
        params: Option<&'a RawValue>,
    },
    /// A response (the server asks the client nothing, so it answers nothing it's sent).
    Response,
}

/// A line that isn't a message.
pub enum Bad {
    /// It isn't JSON (`-32700`): why, with where.
    Parse(String),
    /// It's JSON, but not a request or a notification (`-32600`): its ID, if one could be
    /// read, and why.
    Invalid { id: Option<Id>, why: String },
}

/// A frame's members, each kept as raw text, present or not (`null` is present).
#[derive(Deserialize)]
struct Frame<'a> {
    #[serde(default, borrow, deserialize_with = "present")]
    jsonrpc: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "present")]
    id: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "present")]
    method: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "present")]
    params: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "present")]
    result: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "present")]
    error: Option<&'a RawValue>,
}

/// A member that's there, `null` included (serde's `Option` reads `null` as absent).
pub fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(d).map(Some)
}

/// `line` as a message.
pub fn parse(line: &[u8]) -> Result<Message<'_>, Bad> {
    let text = std::str::from_utf8(line).map_err(|e| {
        Bad::Parse(format!(
            "the message isn't UTF-8: at byte {}",
            e.valid_up_to()
        ))
    })?;
    let whole: &RawValue = serde_json::from_str(text)
        .map_err(|e| Bad::Parse(format!("the message isn't JSON: {e}")))?;
    let invalid = |id: Option<Id>, why: &str| Bad::Invalid {
        id,
        why: why.to_owned(),
    };
    match whole.get().as_bytes().first() {
        Some(b'{') => {}
        Some(b'[') => {
            return Err(invalid(
                None,
                "a batch: MCP 2025-06-18 sends one message a line, never an array of them",
            ));
        }
        _ => return Err(invalid(None, "the message isn't an object")),
    }
    let frame: Frame<'_> = serde_json::from_str(whole.get())
        .map_err(|e| invalid(None, &format!("the message isn't JSON-RPC: {e}")))?;
    let id =
        match frame.id {
            None => None,
            Some(raw) => Some(Id::of(raw).ok_or_else(|| {
                invalid(None, "the message's id is neither a string nor a number")
            })?),
        };
    let version = frame
        .jsonrpc
        .and_then(|raw| serde_json::from_str::<&str>(raw.get()).ok());
    if version != Some("2.0") {
        return Err(invalid(id, "the message's jsonrpc isn't \"2.0\""));
    }
    let Some(method) = frame.method else {
        if id.is_some() && (frame.result.is_some() || frame.error.is_some()) {
            return Ok(Message::Response);
        }
        return Err(invalid(id, "the message has no method"));
    };
    let Ok(method) = serde_json::from_str::<String>(method.get()) else {
        return Err(invalid(id, "the message's method isn't a string"));
    };
    Ok(match id {
        Some(id) => Message::Request {
            id,
            method,
            params: frame.params,
        },
        None => Message::Notification {
            method,
            params: frame.params,
        },
    })
}

/// The ID of the request whose line begins `head` (a line too long to read whole), if it's
/// a member of the line's object before the line was cut: found by walking the object's
/// members, each value skipped unread.
pub fn find_id(head: &[u8]) -> Option<Id> {
    let mut at = Walk { text: head, at: 0 };
    at.space();
    at.byte(b'{')?;
    loop {
        at.space();
        let key = at.string()?;
        at.space();
        at.byte(b':')?;
        at.space();
        let start = at.at;
        at.value()?;
        let value = &head[start..at.at];
        if serde_json::from_slice::<String>(key).ok().as_deref() == Some("id") {
            let raw: &RawValue = serde_json::from_slice(value).ok()?;
            return Id::of(raw);
        }
        at.space();
        at.byte(b',')?;
    }
}

/// A walk through JSON text, value by value, unread.
struct Walk<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Walk<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn byte(&mut self, b: u8) -> Option<()> {
        (self.peek()? == b).then(|| self.at += 1)
    }

    /// A string, its quotes and all.
    fn string(&mut self) -> Option<&'a [u8]> {
        let start = self.at;
        self.byte(b'"')?;
        loop {
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    return Some(&self.text[start..self.at]);
                }
                b'\\' => self.at += 2,
                _ => self.at += 1,
            }
        }
    }

    /// Any value, skipped: an object's or an array's nesting counted, its strings walked.
    fn value(&mut self) -> Option<()> {
        let mut depth: usize = 0;
        loop {
            match self.peek()? {
                b'"' => {
                    self.string()?;
                }
                b'{' | b'[' => {
                    depth += 1;
                    self.at += 1;
                }
                b'}' | b']' => {
                    depth = depth.checked_sub(1)?;
                    self.at += 1;
                }
                b',' if depth == 0 => return Some(()),
                _ => self.at += 1,
            }
            if depth == 0 {
                // A scalar runs to the next comma or the object's end.
                match self.peek() {
                    Some(b',' | b'}' | b' ' | b'\t' | b'\r' | b'\n') => return Some(()),
                    Some(_) => {}
                    None => return None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8], max: usize) -> Vec<String> {
        let mut input = std::io::BufReader::with_capacity(4, input);
        let mut out = Vec::new();
        loop {
            match read_line(&mut input, max) {
                Line::Whole(line) => {
                    out.push(format!("whole {}", String::from_utf8(line).unwrap()))
                }
                Line::TooLong { head, length } => out.push(format!(
                    "long {} {length}",
                    String::from_utf8(head).unwrap()
                )),
                Line::End => return out,
                Line::Failed(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn a_line_past_the_most_read_keeps_only_its_head_and_the_next_is_read_whole() {
        // A buffer of 4 bytes, so each line spans several reads.
        assert_eq!(
            lines(b"abc\r\n0123456789\nxy\n\nlast", 6),
            [
                "whole abc",
                "long 012345 10",
                "whole xy",
                "whole ",
                "whole last"
            ]
        );
        // Exactly the most is whole; one past it isn't.
        assert_eq!(
            lines(b"123456\n1234567\n", 6),
            ["whole 123456", "long 123456 7"]
        );
        assert!(lines(b"", 6).is_empty());
    }

    fn id(text: &str) -> Option<String> {
        find_id(text.as_bytes()).map(|id| id.raw().to_owned())
    }

    #[test]
    fn an_id_is_found_in_a_cut_lines_head_if_its_member_is_whole() {
        assert_eq!(
            id(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"arguments":{"a":"xxx"#),
            Some("5".to_owned())
        );
        assert_eq!(
            id(r#" { "method" : "x", "params": {"id": 1, "s": "}\"{"}, "id" : "a\"b" , "#),
            Some(r#""a\"b""#.to_owned())
        );
        assert_eq!(id(r#"{"id": -2.5e3}"#), Some("-2.5e3".to_owned()));
        // Cut before the ID, or inside it; an ID that's neither a string nor a number.
        assert_eq!(id(r#"{"params":{"arguments":{"a":"xxx"#), None);
        assert_eq!(id(r#"{"id":"abc"#), None);
        assert_eq!(id(r#"{"id":12"#), None);
        assert_eq!(id(r#"{"id":null,"x":1}"#), None);
        assert_eq!(id(r#"["id",1]"#), None);
    }

    #[test]
    fn ids_are_the_same_string_however_escaped_and_the_same_number_as_written() {
        let of = |text: &str| Id::of(serde_json::from_str::<&RawValue>(text).unwrap());
        assert_eq!(of(r#""ab""#), of(r#""ab""#));
        assert_eq!(of(r#""ab""#).unwrap().raw(), r#""ab""#);
        assert_ne!(of("1"), of(r#""1""#));
        assert_ne!(of("1"), of("1.0"));
        assert_eq!(of("null"), None);
        assert_eq!(of("true"), None);
        assert_eq!(of("[1]"), None);
    }
}
