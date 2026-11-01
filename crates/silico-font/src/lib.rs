//! The app's fonts (docs/design/m5.0-app.md, sections 8 and 10; decision 11): Inter Medium
//! for text, and JetBrains Mono Regular for IDs and element names (the UI prototype,
//! docs/design/m5-ui/DESIGN.md, *Type*). Each is under the SIL Open Font License, Version
//! 1.1, its notice beside it. It's a crate of its own so `deny.toml` admits OFL-1.1 for it
//! alone, and no other crate brings a font's licence in unseen.
//!
//! - Inter Medium, as Rerun's `re_ui` 0.38.1 ships it (`data/Inter-Medium.otf`): Copyright
//!   2020 The Inter Project Authors (<https://github.com/rsms/inter>); `OFL.txt`.
//! - JetBrains Mono Regular, from JetBrains' release 2.304
//!   (`JetBrainsMono-2.304.zip`, `fonts/ttf/JetBrainsMono-Regular.ttf`): Copyright 2020 The
//!   JetBrains Mono Project Authors (<https://github.com/JetBrains/JetBrainsMono>);
//!   `JetBrainsMono-OFL.txt`.

/// Inter Medium, as an OpenType file.
pub const INTER_MEDIUM: &[u8] = include_bytes!("../Inter-Medium.otf");

/// Inter's licence notice, the SIL Open Font License 1.1, which ships with the font.
pub const INTER_LICENSE: &str = include_str!("../OFL.txt");

/// JetBrains Mono Regular, as a TrueType file.
pub const JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../JetBrainsMono-Regular.ttf");

/// JetBrains Mono's licence notice, the SIL Open Font License 1.1, which ships with the font.
pub const JETBRAINS_MONO_LICENSE: &str = include_str!("../JetBrainsMono-OFL.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_is_an_opentype_file_and_its_notice_is_the_ofl() {
        // An OpenType font with CFF outlines starts with the tag `OTTO` (the OpenType
        // specification, "Font file").
        assert_eq!(&INTER_MEDIUM[..4], b"OTTO");
        assert!(INTER_LICENSE.contains("SIL OPEN FONT LICENSE Version 1.1"));
        assert!(INTER_LICENSE.contains("The Inter Project Authors"));
    }

    #[test]
    fn the_monospace_is_a_truetype_file_and_its_notice_is_the_ofl() {
        // A font with TrueType outlines starts with the version 0x00010000 (the OpenType
        // specification, "Font file"); its release's file is 273,900 bytes.
        assert_eq!(&JETBRAINS_MONO_REGULAR[..4], [0, 1, 0, 0]);
        assert_eq!(JETBRAINS_MONO_REGULAR.len(), 273_900);
        assert!(JETBRAINS_MONO_LICENSE.contains("SIL OPEN FONT LICENSE Version 1.1"));
        assert!(JETBRAINS_MONO_LICENSE.contains("The JetBrains Mono Project Authors"));
    }
}
