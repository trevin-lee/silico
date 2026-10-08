//! Frames: a 4-byte little-endian payload length, then the payload, a message encoded with
//! `postcard`. Reading never trusts the length: one above the maximum is rejected before
//! anything is allocated, and the payload buffer grows only as bytes actually arrive, so a
//! peer that lies about the length can't make the reader allocate it.

use std::fmt;
use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The largest payload a frame may carry: 512 MiB, room for a large B-rep or mesh.
pub const MAX_FRAME_LEN: u32 = 512 * 1024 * 1024;

#[derive(Debug)]
#[non_exhaustive]
pub enum FrameError {
    /// Reading or writing the stream failed.
    Io(io::Error),
    /// The stream ended partway through a frame.
    Truncated { expected: u64, got: u64 },
    /// A frame's length is above the maximum.
    TooLarge { len: u64, max: u32 },
    /// The payload isn't a valid encoding of the expected message.
    Decode(postcard::Error),
    /// The payload has bytes left over after the message.
    TrailingBytes { extra: usize },
    /// The message couldn't be encoded.
    Encode(postcard::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "frame I/O failed: {e}"),
            FrameError::Truncated { expected, got } => {
                write!(
                    f,
                    "stream ended partway through a frame ({got} of {expected} bytes)"
                )
            }
            FrameError::TooLarge { len, max } => {
                write!(f, "frame of {len} bytes is over the {max}-byte maximum")
            }
            FrameError::Decode(e) => write!(f, "frame isn't a valid message: {e}"),
            FrameError::TrailingBytes { extra } => {
                write!(f, "frame has {extra} bytes after the message")
            }
            FrameError::Encode(e) => write!(f, "message couldn't be encoded: {e}"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FrameError::Io(e) => Some(e),
            FrameError::Decode(e) | FrameError::Encode(e) => Some(e),
            _ => None,
        }
    }
}

/// Encodes `message` and writes it as one frame, then flushes.
pub fn write_message<W: Write, T: Serialize>(
    writer: &mut W,
    message: &T,
) -> Result<(), FrameError> {
    let payload = postcard::to_allocvec(message).map_err(FrameError::Encode)?;
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|len| *len <= MAX_FRAME_LEN)
        .ok_or(FrameError::TooLarge {
            len: payload.len() as u64,
            max: MAX_FRAME_LEN,
        })?;
    writer
        .write_all(&len.to_le_bytes())
        .map_err(FrameError::Io)?;
    writer.write_all(&payload).map_err(FrameError::Io)?;
    writer.flush().map_err(FrameError::Io)
}

/// Reads frames, one at a time, from any byte stream.
pub struct FrameReader<R> {
    inner: R,
    max_len: u32,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_max_len(inner, MAX_FRAME_LEN)
    }

    /// A reader with a lower maximum than [`MAX_FRAME_LEN`].
    pub fn with_max_len(inner: R, max_len: u32) -> Self {
        Self {
            inner,
            max_len: max_len.min(MAX_FRAME_LEN),
        }
    }

    /// The next frame's payload, or `None` if the stream ended cleanly between frames.
    pub fn read_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let mut header = [0u8; 4];
        let mut got = 0;
        while got < header.len() {
            match self.inner.read(&mut header[got..]) {
                Ok(0) if got == 0 => return Ok(None),
                Ok(0) => {
                    return Err(FrameError::Truncated {
                        expected: 4,
                        got: got as u64,
                    });
                }
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(FrameError::Io(e)),
            }
        }
        let len = u32::from_le_bytes(header);
        if len > self.max_len {
            return Err(FrameError::TooLarge {
                len: u64::from(len),
                max: self.max_len,
            });
        }
        let mut payload = Vec::new();
        let read = (&mut self.inner)
            .take(u64::from(len))
            .read_to_end(&mut payload)
            .map_err(FrameError::Io)?;
        if read as u64 != u64::from(len) {
            return Err(FrameError::Truncated {
                expected: u64::from(len) + 4,
                got: read as u64 + 4,
            });
        }
        Ok(Some(payload))
    }

    /// The next message, or `None` if the stream ended cleanly between frames.
    pub fn read_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, FrameError> {
        let Some(payload) = self.read_frame()? else {
            return Ok(None);
        };
        let (message, rest) = postcard::take_from_bytes(&payload).map_err(FrameError::Decode)?;
        if !rest.is_empty() {
            return Err(FrameError::TrailingBytes { extra: rest.len() });
        }
        Ok(Some(message))
    }
}
