//! What an in-process kernel call logs at WARN and above, captured as text: OCCT's warnings
//! reach a caller only as the kernel's log.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `f`'s value, and what it logged at WARN and above on this thread.
pub fn warnings_of<T>(f: impl FnOnce() -> T) -> (T, String) {
    let logs = Captured::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let value = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    (value, text)
}
