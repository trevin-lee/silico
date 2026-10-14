//! Any bytes are read as a document or refused with `invalid_input`: never a panic or a
//! hang. A document read writes canonical text that reads back as itself and writes the
//! same bytes again.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silico_core::ErrorCode;
use silico_core::document::{read, write};

fuzz_target!(|data: &[u8]| {
    match read(data) {
        Ok(document) => {
            let text = write(&document).expect("a document read is written");
            let back = read(text.as_bytes()).expect("canonical text reads back");
            assert_eq!(back, document);
            assert_eq!(write(&back).unwrap(), text);
        }
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            assert!(e.suggestion().is_some());
            // Where, unless it was refused for its size.
            assert!(e.message().contains("line ") || e.message().contains("too large"));
        }
    }
});
