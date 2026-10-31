//! Inter, the app's one font (docs/design/m5.0-app.md, sections 8 and 10; decision 11):
//! Inter Medium, as Rerun's `re_ui` 0.38.1 ships it (`data/Inter-Medium.otf`). The font is
//! Copyright 2020 The Inter Project Authors (<https://github.com/rsms/inter>), licensed
//! under the SIL Open Font License, Version 1.1, whose text is beside it (`OFL.txt`). It's a
//! crate of its own so `deny.toml` admits OFL-1.1 for it alone, and no other crate brings a
//! font's licence in unseen.

/// Inter Medium, as an OpenType file.
pub const INTER_MEDIUM: &[u8] = include_bytes!("../Inter-Medium.otf");

/// Inter's licence notice, the SIL Open Font License 1.1, which ships with the font.
pub const INTER_LICENSE: &str = include_str!("../OFL.txt");

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
}
