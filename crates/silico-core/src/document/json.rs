//! A strict JSON reader for `.silico` documents. It reads standard JSON (RFC 8259), more
//! strictly: no byte-order mark, no duplicate keys, nesting no deeper than [`MAX_DEPTH`],
//! and every number kept as the text it was written as, so lengths and angles are read
//! from their decimals, never through a float. Every value keeps its line and column, for
//! errors that say where.

use std::collections::BTreeSet;
use std::fmt;

/// The deepest arrays and objects may nest. A document nests three deep; the limit keeps
/// hostile input from exhausting the stack.
pub const MAX_DEPTH: usize = 64;

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
}

/// Reads `bytes` as one JSON value, with nothing but whitespace after it.
pub fn read(bytes: &[u8]) -> Result<Node, SyntaxError> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            // Where the first invalid byte is, from the valid text before it.
            let valid = bytes.get(..e.valid_up_to()).unwrap_or_default();
            let at = end_of(&String::from_utf8_lossy(valid));
            return Err(SyntaxError {
                at,
                message: "the text isn't UTF-8".to_owned(),
            });
        }
    };
    let mut reader = Reader {
        chars: text.chars().collect(),
        pos: 0,
        line: 1,
        column: 1,
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

struct Reader {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    column: u32,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn at(&self) -> At {
        At {
            line: self.line,
            column: self.column,
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
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
        }
    }

    /// What's here, for messages: the character, or the end of the text.
    fn found(&self) -> String {
        match self.peek() {
            Some(c) if c.is_control() => format!("the character U+{:04X}", c as u32),
            Some(c) => format!("`{c}`"),
            None => "the end of the text".to_owned(),
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

    fn value(&mut self, depth: usize) -> Result<Node, SyntaxError> {
        let at = self.at();
        let value = match self.peek() {
            Some('{') => self.object(depth)?,
            Some('[') => self.array(depth)?,
            Some('"') => Value::String(self.string()?),
            Some('-' | '0'..='9') => Value::Number(self.number()?),
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
        let mut seen = BTreeSet::new();
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
            if !seen.insert(key.clone()) {
                return Err(SyntaxError {
                    at,
                    message: format!("the key `{key}` is written twice in one object"),
                });
            }
            self.skip_whitespace();
            self.expect(':')?;
            self.skip_whitespace();
            let value = self.value(depth)?;
            members.push(Member { key, at, value });
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    return Ok(Value::Object(members));
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
            items.push(self.value(depth)?);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(Value::Array(items));
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
            ("{\"a\": 1, \"a\": 2}", "`a` is written twice", (1, 10)),
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
}
