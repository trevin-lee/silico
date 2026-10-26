//! Replies, written line by line (docs/design/m4.9-mcp.md, *Exact JSON both ways* and
//! *Running a call*): one lock on the blocking output, each message written whole under it
//! and flushed. A tool's answer splices the core's envelope in raw as `structuredContent`,
//! and streams the same text, escaped, as `content`'s text block; a JSON-RPC error's
//! `data` is a refused envelope, spliced the same way.

use std::io::{self, BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use silico_core::Error;
use silico_core::command::wire::{self, Layout};
use silico_core::command::{Answered, Problems};

use crate::Control;
use crate::frame::Id;

/// JSON-RPC's error codes.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

/// The client's end: the output, under one lock, and what to tell when a write fails.
pub struct Output {
    out: Mutex<BufWriter<Box<dyn Write + Send>>>,
    failed: AtomicBool,
    control: Arc<Control>,
}

impl Output {
    pub fn new(out: Box<dyn Write + Send>, control: Arc<Control>) -> Self {
        Output {
            out: Mutex::new(BufWriter::new(out)),
            failed: AtomicBool::new(false),
            control,
        }
    }

    /// Writes one message (`message` writes it, without its newline), waiting for the
    /// output: false if it couldn't be, the client gone.
    pub fn send(&self, message: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> bool {
        let out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        self.write(out, message)
    }

    /// Writes one message if the output is free now; otherwise drops it (progress, which
    /// is never queued): whether it was written.
    pub fn send_if_free(&self, message: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> bool {
        let out = match self.out.try_lock() {
            Ok(out) => out,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        self.write(out, message)
    }

    fn write(
        &self,
        mut out: MutexGuard<'_, BufWriter<Box<dyn Write + Send>>>,
        message: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> bool {
        if self.failed.load(Ordering::SeqCst) {
            return false;
        }
        let written = message(&mut *out)
            .and_then(|()| out.write_all(b"\n"))
            .and_then(|()| out.flush());
        match written {
            Ok(()) => true,
            Err(e) => {
                self.failed.store(true, Ordering::SeqCst);
                drop(out);
                tracing::info!("the client's end closed: {e}");
                self.control.output_failed();
                false
            }
        }
    }
}

/// A result: `{"jsonrpc": "2.0", "id": ID, "result": RESULT}`, `result` its raw text.
pub fn result<'a>(
    id: &'a Id,
    result: &'a str,
) -> impl FnOnce(&mut dyn Write) -> io::Result<()> + 'a {
    move |out| {
        write!(out, r#"{{"jsonrpc":"2.0","id":{},"result":"#, id.raw())?;
        out.write_all(result.as_bytes())?;
        out.write_all(b"}")
    }
}

/// A JSON-RPC error, its `data` a refused envelope of `error` (or of every error in
/// `problems`): `id` null when none could be read.
pub fn error(id: Option<&Id>, code: i32, message: &str, data: &Problems) -> String {
    let id = id.map_or("null", Id::raw);
    let message = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":{message},"data":{}}}}}"#,
        refused(data)
    )
}

/// A refused envelope of `problems`, on one line, as the core's wire writes it.
pub fn refused(problems: &Problems) -> String {
    wire::write(&wire::refused(None, None, problems), Layout::OneLine)
        .or_else(|_| {
            // A refusal past the core's response limit (no refusal here comes near it).
            let short = Problems::from(Error::new(
                silico_core::ErrorCode::Internal,
                "the refusal was too long to write",
            ));
            wire::write(&wire::refused(None, None, &short), Layout::OneLine)
        })
        .unwrap_or_default()
}

/// A tool's answer: `{"jsonrpc": "2.0", "id": ID, "result": {"content": [the envelope as
/// text, and `render`'s PNG as an image], "structuredContent": the envelope, "isError":
/// !ok}}`.
pub fn answer<'a>(
    id: &'a Id,
    answered: &'a Answered,
) -> impl FnOnce(&mut dyn Write) -> io::Result<()> + 'a {
    move |out| {
        write!(
            out,
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"content":[{{"type":"text","text":"#,
            id.raw()
        )?;
        serde_json::to_writer(&mut *out, answered.text.as_str()).map_err(io::Error::from)?;
        out.write_all(b"}")?;
        if let Some(png) = &answered.png {
            out.write_all(br#",{"type":"image","mimeType":"image/png","data":""#)?;
            base64(out, png)?;
            out.write_all(b"\"}")?;
        }
        out.write_all(br#"],"structuredContent":"#)?;
        out.write_all(answered.text.as_bytes())?;
        write!(out, r#","isError":{}}}}}"#, !answered.ok)
    }
}

/// A progress notification: the call's count so far, and the step running.
pub fn progress<'a>(
    token: &'a str,
    count: u64,
    running: Option<&'a str>,
) -> impl FnOnce(&mut dyn Write) -> io::Result<()> + 'a {
    move |out| {
        write!(
            out,
            r#"{{"jsonrpc":"2.0","method":"notifications/progress","params":{{"progressToken":{token},"progress":{count}"#
        )?;
        if let Some(running) = running {
            out.write_all(br#","message":"#)?;
            serde_json::to_writer(&mut *out, running).map_err(io::Error::from)?;
        }
        out.write_all(b"}}")
    }
}

/// A message already written, as text.
pub fn text(text: &str) -> impl FnOnce(&mut dyn Write) -> io::Result<()> + '_ {
    move |out| out.write_all(text.as_bytes())
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` in base64 (RFC 4648, section 4, padded), streamed to `out` a block at a time.
pub fn base64(out: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
    let mut block = [0u8; 4096];
    for chunk in bytes.chunks(3 * (4096 / 4)) {
        let mut at = 0;
        for group in chunk.chunks(3) {
            let b = [
                group[0],
                group.get(1).copied().unwrap_or(0),
                group.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            let digit = |shift: u32| BASE64[((n >> shift) & 63) as usize];
            block[at] = digit(18);
            block[at + 1] = digit(12);
            block[at + 2] = if group.len() > 1 { digit(6) } else { b'=' };
            block[at + 3] = if group.len() > 2 { digit(0) } else { b'=' };
            at += 4;
        }
        out.write_all(&block[..at])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(bytes: &[u8]) -> String {
        let mut out = Vec::new();
        base64(&mut out, bytes).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn base64_is_rfc_4648s() {
        // RFC 4648, section 10's test vectors.
        for (bytes, text) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encoded(bytes.as_bytes()), text);
        }
        // Every digit, and a length past a block: 3,072 bytes a block, written as 4,096.
        assert_eq!(encoded(&[0xfb, 0xff, 0xbf]), "+/+/");
        let long: Vec<u8> = (0..3 * 1024 + 2).map(|i| (i % 3) as u8).collect();
        let text = encoded(&long);
        assert_eq!(text.len(), 4 * 1025);
        // 00 01 02 is AAEC; the last two, 00 01, are AAE=.
        assert!(text[..4096].bytes().eq("AAEC".repeat(1024).bytes()));
        assert_eq!(&text[4096..], "AAE=");
    }
}
