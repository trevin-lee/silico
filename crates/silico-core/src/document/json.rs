//! A strict JSON reader for `.silico` documents. It reads standard JSON (RFC 8259), more
//! strictly: no byte-order mark, no duplicate keys, nesting no deeper than [`MAX_DEPTH`],
//! at most [`MAX_INPUT_LEN`] bytes read into a tree of at most [`MAX_TREE_BYTES`], and
//! every number kept as the text it was written as, so lengths and angles are read from
//! their decimals, never through a float. Every value keeps its line and column, for
//! errors that say where.

use std::fmt;
use std::fmt::Write as _;
use std::mem::size_of;

/// The deepest arrays and objects may nest. A document nests four deep (a feature's
/// position); the limit keeps hostile input from exhausting the stack.
pub const MAX_DEPTH: usize = 64;

/// The most bytes a document's canonical text may be: 16 MiB, past any part's timeline (a
/// hundred thousand features). Measured on the canonical text, stored fingerprints and
/// all, not the bytes read, so what opens can be written back as it is and what's written
/// opens (`document::read`, `document::write`); a save, which writes the fingerprints and
/// solved sketches its rebuild made, can pass it, and is refused (`command::format`).
pub const MAX_DOCUMENT_LEN: usize = 16 << 20;

/// The most bytes read: four times [`MAX_DOCUMENT_LEN`], room for a document spelled more
/// tightly or loosely than canonical form, and a bound on the time reading takes.
pub const MAX_INPUT_LEN: usize = 4 * MAX_DOCUMENT_LEN;

/// The most memory the tree of a document's values may take: 256 MiB, counted as it's
/// built (each value, key, and text, and each array's and object's room), so reading any
/// text costs about that at most (each allocation is charged [`ALLOCATION`] bytes more
/// than it holds; an allocator whose smallest block is larger, as glibc's 32, costs a
/// little more). A real document takes about 7 times its size (a feature's 150
/// bytes are about a kilobyte of tree), so every document within [`MAX_DOCUMENT_LEN`]
/// fits; the text that costs most for its size (`[{"a":0},...]`) doesn't, and is refused
/// when the tree would pass it.
pub const MAX_TREE_BYTES: usize = 256 << 20;

/// The most characters of a string, key, or number an error repeats: enough to recognize
/// it.
pub(crate) const SHOWN_CHARS: usize = 40;

/// What a heap allocation costs beyond its bytes, as charged: an allocator's smallest
/// block and its bookkeeping.
const ALLOCATION: usize = 16;

/// `text` as an error shows it: quoted, with control and invisible characters escaped, and
/// cut short (`…`) if it's long, so no string in a document makes its error unreadable or
/// unsafe to print. Escaped as Rust's `{:?}` escapes a string (`\n`, `\"`, `\u{202e}`),
/// and every other character [`unseen`] holds too (a Hangul filler, a space that isn't the
/// plain one), which `{:?}` leaves raw: so it escapes all that `command::quoted` does.
pub fn shown(text: &str) -> String {
    let mut chars = text.chars();
    let start: String = chars.by_ref().take(SHOWN_CHARS).collect();
    let cut = chars.next().is_some();
    let debug = format!("{start:?}");
    let mut out = String::with_capacity(debug.len() + '…'.len_utf8());
    // `{:?}`'s escapes are ASCII, so a character `unseen` holds is one it left raw.
    for c in debug.chars() {
        if unseen(c) {
            // Writing to a String can't fail.
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        } else {
            out.push(c);
        }
    }
    if cut {
        out.push('…');
    }
    out
}

/// Whether `c` is a character a message shows by its code point, never raw: a control, the
/// line or paragraph separator, every character Unicode draws as nothing ([`invisible`]),
/// and every space but the plain one, which would look like it.
pub(crate) fn unseen(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}')
        || (c.is_whitespace() && c != ' ')
        || invisible(c)
}

/// Where a value starts: a line and a column, both from 1, the column in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct At {
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.column)
    }
}

impl At {
    /// Where a value no one wrote is: one an edit made, or took apart and put back
    /// together ([`unplace`]), whose line and column in a text would name a text no one
    /// has. Line 0, which no value read is at, so an error about it says no place.
    pub const UNPLACED: At = At { line: 0, column: 0 };
}

/// `node`, and every value and key in it, at [`At::UNPLACED`]: so a tree an edit made, of
/// a request's values and a document's, is read with errors that name fields by their
/// paths alone. An explicit stack, so a tree of any depth is walked.
pub fn unplace(node: &mut Node) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        node.at = At::UNPLACED;
        match &mut node.value {
            Value::Array(items) => stack.extend(items.iter_mut()),
            Value::Object(members) => {
                for member in members {
                    member.at = At::UNPLACED;
                    stack.push(&mut member.value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
}

/// A JSON value with no place in a text: what a command gives back of a document's
/// values (`set_field`'s field before and after), each object's members in the order the
/// document writes them and each number as its text writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plain {
    Null,
    Bool(bool),
    /// The number's text, as written.
    Number(String),
    String(String),
    Array(Vec<Plain>),
    /// Members in order, no key twice.
    Object(Vec<(String, Plain)>),
}

impl Plain {
    /// What `node` holds, without where it was.
    pub fn of(node: &Node) -> Plain {
        match &node.value {
            Value::Null => Plain::Null,
            Value::Bool(b) => Plain::Bool(*b),
            Value::Number(text) => Plain::Number(text.clone()),
            Value::String(text) => Plain::String(text.clone()),
            Value::Array(items) => Plain::Array(items.iter().map(Plain::of).collect()),
            Value::Object(members) => Plain::Object(
                members
                    .iter()
                    .map(|m| (m.key.clone(), Plain::of(&m.value)))
                    .collect(),
            ),
        }
    }
}

/// A JSON value, where it starts, and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub at: At,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number's text, as written (valid JSON number syntax).
    Number(String),
    String(String),
    Array(Vec<Node>),
    /// Members in the order written, each key where it starts. No key twice.
    Object(Vec<Member>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub key: String,
    pub at: At,
    pub value: Node,
}

impl Value {
    /// The text, if it's a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// What kind of value it is, for messages: `a number`, `an object`, ...
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "true or false",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }
}

/// Why the bytes aren't a document's JSON, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub at: At,
    pub message: String,
    /// Why: the text's JSON, or its size.
    pub cause: Cause,
}

/// Why the reader refused a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// Its JSON: not JSON, or more than Silico takes (a key twice, a byte-order mark).
    Text,
    /// Its size, or its tree's: more than Silico reads, though it may be valid JSON.
    Size,
}

/// Reads `bytes` as one JSON value, with nothing but whitespace after it.
pub fn read(bytes: &[u8]) -> Result<Node, SyntaxError> {
    if bytes.len() > MAX_INPUT_LEN {
        return Err(SyntaxError {
            at: At { line: 1, column: 1 },
            message: format!(
                "the text is {} bytes, more than the {} Silico reads",
                bytes.len(),
                MAX_INPUT_LEN
            ),
            cause: Cause::Size,
        });
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            // Where the first invalid byte is, from the valid text before it.
            let valid = bytes.get(..e.valid_up_to()).unwrap_or_default();
            let at = end_of(&String::from_utf8_lossy(valid));
            return Err(SyntaxError {
                at,
                message: "the text isn't UTF-8".to_owned(),
                cause: Cause::Text,
            });
        }
    };
    let mut reader = Reader {
        text,
        pos: 0,
        line: 1,
        column: 1,
        spent: size_of::<Node>(),
    };
    if reader.peek() == Some('\u{feff}') {
        return Err(reader.error("the text starts with a byte-order mark"));
    }
    reader.skip_whitespace();
    let node = reader.value(0)?;
    reader.skip_whitespace();
    if reader.peek().is_some() {
        return Err(reader.error("there's more after the document's closing brace"));
    }
    Ok(node)
}

/// Where the end of `text` is.
fn end_of(text: &str) -> At {
    let mut at = At { line: 1, column: 1 };
    for c in text.chars() {
        if c == '\n' {
            at.line = at.line.saturating_add(1);
            at.column = 1;
        } else {
            at.column = at.column.saturating_add(1);
        }
    }
    at
}

struct Reader<'a> {
    text: &'a str,
    /// The byte offset of the next character.
    pos: usize,
    line: u32,
    column: u32,
    /// The tree's bytes so far, as charged against [`MAX_TREE_BYTES`].
    spent: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.text.get(self.pos..)?.chars().next()
    }

    fn at(&self) -> At {
        At {
            line: self.line,
            column: self.column,
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line = self.line.saturating_add(1);
            self.column = 1;
        } else {
            self.column = self.column.saturating_add(1);
        }
        Some(c)
    }

    fn error(&self, message: impl Into<String>) -> SyntaxError {
        SyntaxError {
            at: self.at(),
            message: message.into(),
            cause: Cause::Text,
        }
    }

    /// What's here, for messages: the character, or the end of the text. A character
    /// that can't be seen (a control, a space other than ASCII's, a format character such
    /// as a zero-width space or a byte-order mark) is named by its code point alone, and
    /// any other but ASCII's with its code point too.
    fn found(&self) -> String {
        match self.peek() {
            None => "the end of the text".to_owned(),
            Some(c) if c.is_ascii_graphic() => format!("`{c}`"),
            Some(c) if c.is_control() || c.is_whitespace() || invisible(c) => {
                format!("the character U+{:04X}", c as u32)
            }
            Some(c) => format!("`{c}` (U+{:04X})", c as u32),
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(' ' | '\t' | '\n' | '\r') = self.peek() {
            self.bump();
        }
    }

    fn expect(&mut self, want: char) -> Result<(), SyntaxError> {
        if self.peek() == Some(want) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(format!("expected `{want}`, found {}", self.found())))
        }
    }

    /// Charges `bytes` of the tree, or refuses the document if they'd pass the limit.
    fn charge(&mut self, bytes: usize) -> Result<(), SyntaxError> {
        self.spent = self.spent.saturating_add(bytes);
        if self.spent > MAX_TREE_BYTES {
            return Err(SyntaxError {
                cause: Cause::Size,
                ..self.error(format!(
                    "reading the document would take more than the {MAX_TREE_BYTES} bytes of memory a document may"
                ))
            });
        }
        Ok(())
    }

    /// Room in `items` for one more, charged as it's made.
    fn reserve_one<T>(&mut self, items: &mut Vec<T>) -> Result<(), SyntaxError> {
        if items.len() == items.capacity() {
            let more = items.capacity().max(4);
            let charge = if items.capacity() == 0 {
                more * size_of::<T>() + ALLOCATION
            } else {
                more * size_of::<T>()
            };
            self.charge(charge)?;
            items.reserve_exact(more);
        }
        Ok(())
    }

    /// `items` with no room to spare, its spare room given back to the charge.
    fn shrink_charged<T>(&mut self, mut items: Vec<T>) -> Vec<T> {
        let spare = items.capacity() - items.len();
        self.spent = self.spent.saturating_sub(spare * size_of::<T>());
        items.shrink_to_fit();
        items
    }

    /// `text` with no room to spare, charged.
    fn charge_text(&mut self, mut text: String) -> Result<String, SyntaxError> {
        text.shrink_to_fit();
        if !text.is_empty() {
            self.charge(text.len() + ALLOCATION)?;
        }
        Ok(text)
    }

    fn value(&mut self, depth: usize) -> Result<Node, SyntaxError> {
        let at = self.at();
        let value = match self.peek() {
            Some('{') => self.object(depth)?,
            Some('[') => self.array(depth)?,
            Some('"') => {
                let text = self.string()?;
                Value::String(self.charge_text(text)?)
            }
            Some('-' | '0'..='9') => {
                let text = self.number()?;
                Value::Number(self.charge_text(text)?)
            }
            Some('t') => self.word("true", Value::Bool(true))?,
            Some('f') => self.word("false", Value::Bool(false))?,
            Some('n') => self.word("null", Value::Null)?,
            _ => return Err(self.error(format!("expected a value, found {}", self.found()))),
        };
        Ok(Node { at, value })
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, SyntaxError> {
        let start = self.at();
        for want in word.chars() {
            if self.peek() != Some(want) {
                return Err(SyntaxError {
                    at: start,
                    message: format!("expected a value, found {}", self.found()),
                    cause: Cause::Text,
                });
            }
            self.bump();
        }
        Ok(value)
    }

    fn nested(&self, depth: usize) -> Result<usize, SyntaxError> {
        if depth >= MAX_DEPTH {
            return Err(self.error(format!(
                "arrays and objects nest more than {MAX_DEPTH} deep"
            )));
        }
        Ok(depth + 1)
    }

    fn object(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        let depth = self.nested(depth)?;
        self.expect('{')?;
        self.skip_whitespace();
        let mut members = Vec::new();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_whitespace();
            let at = self.at();
            if self.peek() != Some('"') {
                return Err(self.error(format!("expected a key, found {}", self.found())));
            }
            let key = self.string()?;
            let key = self.charge_text(key)?;
            self.skip_whitespace();
            self.expect(':')?;
            self.skip_whitespace();
            let value = self.value(depth)?;
            self.reserve_one(&mut members)?;
            members.push(Member { key, at, value });
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    refuse_repeated_key(&members)?;
                    return Ok(Value::Object(self.shrink_charged(members)));
                }
                _ => {
                    return Err(self.error(format!("expected `,` or `}}`, found {}", self.found())));
                }
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, SyntaxError> {
        let depth = self.nested(depth)?;
        self.expect('[')?;
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            let item = self.value(depth)?;
            self.reserve_one(&mut items)?;
            items.push(item);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(Value::Array(self.shrink_charged(items)));
                }
                _ => {
                    return Err(self.error(format!("expected `,` or `]`, found {}", self.found())));
                }
            }
        }
    }

    /// A number's text, checked against JSON's grammar:
    /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`.
    fn number(&mut self) -> Result<String, SyntaxError> {
        let mut text = String::new();
        if self.peek() == Some('-') {
            text.extend(self.bump());
        }
        match self.peek() {
            Some('0') => text.extend(self.bump()),
            Some('1'..='9') => self.digits(&mut text),
            _ => {
                return Err(self.error(format!(
                    "expected a digit in a number, found {}",
                    self.found()
                )));
            }
        }
        if self.peek() == Some('.') {
            text.extend(self.bump());
            self.required_digits(&mut text, "after a number's decimal point")?;
        }
        if let Some('e' | 'E') = self.peek() {
            text.extend(self.bump());
            if let Some('+' | '-') = self.peek() {
                text.extend(self.bump());
            }
            self.required_digits(&mut text, "in a number's exponent")?;
        }
        if let Some('0'..='9') = self.peek() {
            // A leading zero followed by more digits (`01`).
            return Err(self.error("a number can't start with 0 followed by more digits"));
        }
        Ok(text)
    }

    fn digits(&mut self, text: &mut String) {
        while let Some('0'..='9') = self.peek() {
            text.extend(self.bump());
        }
    }

    fn required_digits(&mut self, text: &mut String, place: &str) -> Result<(), SyntaxError> {
        if !matches!(self.peek(), Some('0'..='9')) {
            return Err(self.error(format!("expected a digit {place}, found {}", self.found())));
        }
        self.digits(text);
        Ok(())
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("a string isn't closed")),
                Some('"') => {
                    self.bump();
                    return Ok(out);
                }
                Some('\\') => {
                    self.bump();
                    out.push(self.escape()?);
                }
                Some(c) if (c as u32) < 0x20 => {
                    return Err(self.error(format!(
                        "a string holds the control character U+{:04X}: write it as an escape",
                        c as u32
                    )));
                }
                Some(c) => {
                    self.bump();
                    out.push(c);
                }
            }
        }
    }

    /// The character an escape after `\` stands for.
    fn escape(&mut self) -> Result<char, SyntaxError> {
        let c = match self.peek() {
            Some('"') => '"',
            Some('\\') => '\\',
            Some('/') => '/',
            Some('b') => '\u{8}',
            Some('f') => '\u{c}',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('u') => {
                self.bump();
                return self.unicode_escape();
            }
            _ => return Err(self.error(format!("`\\` is followed by {}", self.found()))),
        };
        self.bump();
        Ok(c)
    }

    /// A `\uXXXX` escape (the `\u` read), with a surrogate pair's second half.
    fn unicode_escape(&mut self) -> Result<char, SyntaxError> {
        let first = self.hex4()?;
        match first {
            0xd800..=0xdbff => {
                if self.peek() != Some('\\') {
                    return Err(
                        self.error("a high surrogate escape isn't followed by its low half")
                    );
                }
                self.bump();
                if self.peek() != Some('u') {
                    return Err(
                        self.error("a high surrogate escape isn't followed by its low half")
                    );
                }
                self.bump();
                let second = self.hex4()?;
                if !(0xdc00..=0xdfff).contains(&second) {
                    return Err(
                        self.error("a high surrogate escape isn't followed by its low half")
                    );
                }
                let code = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                char::from_u32(code).ok_or_else(|| self.error("an escape isn't a character"))
            }
            0xdc00..=0xdfff => Err(self.error("a low surrogate escape has no high half before it")),
            _ => char::from_u32(first).ok_or_else(|| self.error("an escape isn't a character")),
        }
    }

    fn hex4(&mut self) -> Result<u32, SyntaxError> {
        let mut code = 0u32;
        for _ in 0..4 {
            let digit = self.peek().and_then(|c| c.to_digit(16)).ok_or_else(|| {
                self.error(format!("expected a hex digit, found {}", self.found()))
            })?;
            self.bump();
            code = code * 16 + digit;
        }
        Ok(code)
    }
}

/// Refuses an object with a key written twice, at the second writing of the first key to
/// be (in the order written). Found by sorting, so the keys aren't kept twice.
fn refuse_repeated_key(members: &[Member]) -> Result<(), SyntaxError> {
    let mut order: Vec<usize> = (0..members.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |i: usize| members.get(i).map(|m| m.key.as_str());
        key(a).cmp(&key(b)).then(a.cmp(&b))
    });
    let again = order
        .windows(2)
        .filter_map(|pair| {
            let (first, second) = (members.get(pair[0])?, members.get(pair[1])?);
            (first.key == second.key).then_some(pair[1])
        })
        .min();
    match again.and_then(|i| members.get(i)) {
        Some(member) => Err(SyntaxError {
            at: member.at,
            message: format!(
                "the key {} is written twice in one object",
                shown(&member.key)
            ),
            cause: Cause::Text,
        }),
        None => Ok(()),
    }
}

/// Whether `c` is a bidirectional control: the marks, embeddings, overrides, and isolates,
/// which reorder how the text around them shows (Trojan Source, CVE-2021-42574). The
/// writer escapes them; errors name them by code point.
pub(crate) fn bidi_control(c: char) -> bool {
    matches!(
        c as u32,
        0x61c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069
    )
}

/// Whether `c` is a format or other invisible character Unicode draws as nothing: the
/// bidirectional controls, soft hyphen, combining grapheme joiner, the zero-width
/// characters, word joiners and invisible operators, variation selectors, the byte-order
/// mark, the Hangul fillers, the tag characters, and the rest of Unicode's format
/// characters (category Cf: Arabic and Syriac number signs and marks, the Mongolian vowel
/// separator, the interlinear annotation characters, Kaithi and Egyptian hieroglyph
/// format controls, shorthand format controls, musical symbol beams).
pub(crate) fn invisible(c: char) -> bool {
    bidi_control(c)
        || matches!(
            c as u32,
            0xad | 0x34f
                | 0x115f..=0x1160
                | 0x17b4..=0x17b5
                | 0x180b..=0x180f
                | 0x200b..=0x200d
                | 0x2060..=0x2065
                | 0x206a..=0x206f
                | 0x3164
                | 0xfe00..=0xfe0f
                | 0xfeff
                | 0xffa0
                | 0xfff0..=0xfffb
                | 0x600..=0x605
                | 0x6dd
                | 0x70f
                | 0x890..=0x891
                | 0x8e2
                | 0x110bd
                | 0x110cd
                | 0x13430..=0x1343f
                | 0x1bca0..=0x1bca3
                | 0x1d173..=0x1d17a
                | 0xe0000..=0xe0fff
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_str(text: &str) -> Result<Node, SyntaxError> {
        read(text.as_bytes())
    }

    #[test]
    fn numbers_are_kept_as_written() {
        let node = read_str(r#"[0.1, -2.50, 1e-3, 0]"#).unwrap();
        let Value::Array(items) = node.value else {
            panic!()
        };
        let texts: Vec<_> = items
            .into_iter()
            .map(|n| match n.value {
                Value::Number(t) => t,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(texts, ["0.1", "-2.50", "1e-3", "0"]);
    }

    #[test]
    fn values_know_where_they_start() {
        let node = read_str("{\n  \"a\": [1,\n    \"é\", true]\n}").unwrap();
        assert_eq!(node.at, At { line: 1, column: 1 });
        let Value::Object(members) = node.value else {
            panic!()
        };
        assert_eq!(members[0].at, At { line: 2, column: 3 });
        let Value::Array(items) = &members[0].value.value else {
            panic!()
        };
        assert_eq!(items[0].at, At { line: 2, column: 9 });
        assert_eq!(items[1].at, At { line: 3, column: 5 });
        // Columns count characters, not bytes: `é` is one.
        assert_eq!(
            items[2].at,
            At {
                line: 3,
                column: 10
            }
        );
    }

    #[test]
    fn escapes_read_as_the_characters_they_stand_for() {
        let node = read_str(r#""a\"\\\/\b\f\n\r\t\u00e9\ud83d\ude00""#).unwrap();
        assert_eq!(
            node.value,
            Value::String("a\"\\/\u{8}\u{c}\n\r\té😀".to_owned())
        );
    }

    #[test]
    fn what_json_forbids_or_silico_refuses_is_refused_saying_where() {
        let cases: &[(&str, &str, (u32, u32))] = &[
            ("", "expected a value, found the end", (1, 1)),
            ("\u{feff}{}", "byte-order mark", (1, 1)),
            ("{} {}", "more after", (1, 4)),
            (
                "{\"a\": 1, \"a\": 2}",
                "key \"a\" is written twice",
                (1, 10),
            ),
            ("[1, 2,]", "expected a value, found `]`", (1, 7)),
            ("{\"a\": 1,}", "expected a key, found `}`", (1, 9)),
            ("[01]", "start with 0", (1, 3)),
            ("[1.]", "after a number's decimal point", (1, 4)),
            ("[-]", "expected a digit in a number", (1, 3)),
            ("[1e]", "in a number's exponent", (1, 4)),
            ("[NaN]", "expected a value, found `N`", (1, 2)),
            ("[Infinity]", "expected a value, found `I`", (1, 2)),
            ("[tru]", "expected a value", (1, 2)),
            ("\"a\nb\"", "control character U+000A", (1, 3)),
            ("\"\\x\"", "`\\` is followed by `x`", (1, 3)),
            ("\"\\ud83d\"", "low half", (1, 8)),
            ("\"\\ude00\"", "no high half", (1, 8)),
            ("\"\\u12g4\"", "hex digit", (1, 6)),
            ("\"abc", "isn't closed", (1, 5)),
            ("// note\n{}", "expected a value, found `/`", (1, 1)),
            ("{'a': 1}", "expected a key, found `'`", (1, 2)),
        ];
        for &(text, says, (line, column)) in cases {
            let err = read_str(text).unwrap_err();
            assert!(err.message.contains(says), "{text:?}: {err:?}");
            assert_eq!(err.at, At { line, column }, "{text:?}: {err:?}");
        }
    }

    #[test]
    fn bytes_that_arent_utf8_are_refused_saying_where() {
        let err = read(b"{\n  \"a\": \"\xff\"}").unwrap_err();
        assert_eq!(err.message, "the text isn't UTF-8");
        assert_eq!(err.at, At { line: 2, column: 9 });
    }

    #[test]
    fn nesting_past_the_limit_is_refused_not_overflowed() {
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(read_str(&ok).is_ok());
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        let err = read_str(&deep).unwrap_err();
        assert!(err.message.contains("nest more than 64"), "{err:?}");
        // Far deeper: refused at the limit, without recursing further.
        let hostile = "[".repeat(1_000_000);
        assert!(read_str(&hostile).is_err());
        let objects = "{\"a\":".repeat(1_000_000);
        assert!(read_str(&objects).is_err());
    }

    #[test]
    fn a_character_ascii_shows_is_named_by_itself_alone() {
        // Whole messages: a printable ASCII character is shown as itself, without its code
        // point; any other, with it.
        assert_eq!(
            read_str("x").unwrap_err().message,
            "expected a value, found `x`"
        );
        assert_eq!(
            read_str("[1 ~]").unwrap_err().message,
            "expected `,` or `]`, found `~`"
        );
        assert_eq!(
            read_str("é").unwrap_err().message,
            "expected a value, found `é` (U+00E9)"
        );
    }

    fn reader(text: &str) -> Reader<'_> {
        Reader {
            text,
            pos: 0,
            line: 1,
            column: 1,
            spent: size_of::<Node>(),
        }
    }

    #[test]
    fn an_arrays_room_is_charged_as_it_grows_and_its_spare_room_given_back() {
        // By hand, with N a node's bytes: the root's N; room for 4 items at the first
        // (4N, and an allocation's 16); room for 4 more at the fifth (4N); and the 3 left
        // spare at the end given back (-3N). 6N + 16: the root and its 5 items, and one
        // allocation.
        let n = size_of::<Node>();
        let mut r = reader("[true,true,true,true,true]");
        let Value::Array(items) = r.value(0).unwrap().value else {
            panic!()
        };
        assert_eq!((items.len(), items.capacity()), (5, 5));
        assert_eq!(r.spent, 6 * n + 16);
        // Four fit the first room: nothing given back, nothing more charged.
        let mut r = reader("[true,true,true,true]");
        r.value(0).unwrap();
        assert_eq!(r.spent, 5 * n + 16);
    }

    #[test]
    fn a_tree_may_take_exactly_the_most_bytes_and_not_one_more() {
        let mut r = reader("");
        r.spent = MAX_TREE_BYTES - 10;
        assert!(r.charge(10).is_ok());
        assert_eq!(r.spent, MAX_TREE_BYTES);
        let err = r.charge(1).unwrap_err();
        assert_eq!(err.cause, Cause::Size);
    }

    #[test]
    fn a_string_an_error_shows_escapes_every_character_quoted_does() {
        // By hand: Rust's `{:?}` escapes the controls and the bidirectional overrides, but
        // left the Hangul fillers (U+3164, U+115F, letters to Unicode, drawn as nothing)
        // and the spaces that aren't the plain one raw, which `command::quoted` escapes.
        for (text, want) in [
            ("a\u{3164}b", r#""a\u{3164}b""#),
            ("\u{115f}", r#""\u{115f}""#),
            ("\u{ffa0}x", r#""\u{ffa0}x""#),
            ("no\u{a0}break", r#""no\u{a0}break""#),
            ("em\u{2003}space", r#""em\u{2003}space""#),
            ("ideographic\u{3000}", r#""ideographic\u{3000}""#),
            ("tag\u{e0041}", r#""tag\u{e0041}""#),
            // `{:?}`'s own escapes are kept as they were.
            ("a\nb\t\"c\\", r#""a\nb\t\"c\\""#),
            ("evil\u{202e}txt", r#""evil\u{202e}txt""#),
            ("zero\u{200b}width", r#""zero\u{200b}width""#),
            ("plain spaces and é", r#""plain spaces and é""#),
        ] {
            assert_eq!(shown(text), want, "{text:?}");
            // No character it leaves raw is one `quoted` escapes.
            assert!(shown(text).chars().all(|c| !unseen(c)), "{text:?}");
        }
        // Cut at 40 characters, each counted once, escaped or not.
        let long = "\u{3164}".repeat(41);
        let want = format!("\"{}\"…", r"\u{3164}".repeat(40));
        assert_eq!(shown(&long), want);
    }

    #[test]
    fn every_format_character_is_invisible() {
        // Found by M2.9's tests review: `invisible` says it holds every format character
        // (Unicode's category Cf), but only a few were tested. These are all 170 of
        // Unicode 16.0's, from the Unicode database (Python's unicodedata, category "Cf").
        const FORMAT: &[(u32, u32)] = &[
            (0xad, 0xad),
            (0x600, 0x605),
            (0x61c, 0x61c),
            (0x6dd, 0x6dd),
            (0x70f, 0x70f),
            (0x890, 0x891),
            (0x8e2, 0x8e2),
            (0x180e, 0x180e),
            (0x200b, 0x200f),
            (0x202a, 0x202e),
            (0x2060, 0x2064),
            (0x2066, 0x206f),
            (0xfeff, 0xfeff),
            (0xfff9, 0xfffb),
            (0x110bd, 0x110bd),
            (0x110cd, 0x110cd),
            (0x13430, 0x1343f),
            (0x1bca0, 0x1bca3),
            (0x1d173, 0x1d17a),
            (0xe0001, 0xe0001),
            (0xe0020, 0xe007f),
        ];
        let mut count = 0;
        for &(first, last) in FORMAT {
            for code in first..=last {
                let c = char::from_u32(code).unwrap();
                assert!(invisible(c), "U+{code:04X}");
                count += 1;
            }
        }
        assert_eq!(count, 170);
        // Characters that are seen aren't.
        for c in ['a', '-', '\u{e9}', '\u{2010}', '\u{5d0}', '\u{4e00}'] {
            assert!(!invisible(c), "{c:?}");
        }
    }
}
