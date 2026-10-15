//! Any text is read as a length and an angle or refused: never a panic. A value read
//! writes as a decimal that reads back as it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use silico_core::document::{Angle, Length};

fuzz_target!(|text: &str| {
    if let Ok(length) = Length::parse(text) {
        assert_eq!(Length::parse(&length.to_string()), Ok(length));
        let _ = length.mm();
    }
    if let Ok(angle) = Angle::parse(text) {
        assert_eq!(Angle::parse(&angle.to_string()), Ok(angle));
        let _ = angle.degrees();
    }
});
