//! The strings a STEP file carries (ISO 10303-21:2002, 6.4.3), encoded by Silico before
//! OCCT sees them (docs/ports/step-export.md, *Strings*). OCCT's writer doubles `'` and
//! copies every other byte, passing through anything that looks like a directive (`\X2\`,
//! `\N\`, ...), so a name holding one would be read back as something else. Every string the
//! core sends is in [`step_encoded`]'s form, which OCCT writes unchanged but for `'`, and
//! which every reader reads back as the text it encodes; the worker refuses any other
//! ([`is_step_encoded`]).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::{Blob, InvalidParameter, ShapeData};

/// A STEP export (docs/ports/step-export.md, *The worker's request*): each part a product,
/// in the file's order, under an assembly named `assembly` if there are two or more; the
/// header's FILE_NAME `file_name`, its other fields the worker's constants. Every string is
/// in [`step_encoded`]'s form ([`ExportStep::check`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportStep {
    pub parts: Vec<StepPart>,
    /// The assembly's product name: the document's file stem.
    pub assembly: String,
    /// The header's FILE_NAME: the document's file name, its last component only.
    pub file_name: String,
}

/// One part of a STEP export: a body's solid, its product's id (the feature's ID) and its
/// product's name (the feature's name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepPart {
    pub shape: ShapeData,
    pub id: String,
    pub name: String,
}

/// A STEP export written: the file's bytes, and, for each part, in order, the faces left out
/// of it as bounding no material (OCCT's INTERNAL and EXTERNAL, which its writer drops), as
/// numbered in `TopExp::MapShapes` order from 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepFile {
    pub step: Blob,
    pub left_out: Vec<Vec<u32>>,
}

impl ExportStep {
    /// Nothing, if the export can be written: at least one part, and every string in
    /// [`step_encoded`]'s form (the core encodes them all, so another is a fault of the
    /// core's, refused before OCCT reads it as a directive).
    pub fn check(&self) -> Result<(), InvalidParameter> {
        if self.parts.is_empty() {
            return Err(InvalidParameter {
                parameter: "parts",
                message: "a STEP export needs at least one part, and there are none".to_owned(),
            });
        }
        let encoded = |parameter: &'static str, text: &str| {
            if is_step_encoded(text) {
                Ok(())
            } else {
                Err(InvalidParameter {
                    parameter,
                    message: format!(
                        "the {parameter} {text:?} isn't encoded as a STEP string: every \
                         character but printable ASCII in \\X2\\ or \\X4\\ runs"
                    ),
                })
            }
        };
        encoded("assembly", &self.assembly)?;
        encoded("file_name", &self.file_name)?;
        for part in &self.parts {
            encoded("id", &part.id)?;
            encoded("name", &part.name)?;
        }
        Ok(())
    }
}

/// `text` as a STEP string's contents: each character from U+0020 to U+007E but `\` as
/// itself, and every other (`\`, control characters, everything past ASCII), in maximal
/// runs, as `\X2\` and four uppercase hex digits each, then `\X0\`, or, for a character past
/// U+FFFF, `\X4\` and eight (`\X2\` is UCS-2, so it holds no surrogate pairs). Each text has
/// one encoding, and every text has one: nothing is refused for its characters.
pub fn step_encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // The directive of the run open, if one is: `2` or `4`.
    let mut open: Option<u8> = None;
    for c in text.chars() {
        let wide = match c {
            '\\' => Some(2),
            ' '..='~' => None,
            c if (c as u32) <= 0xFFFF => Some(2),
            _ => Some(4),
        };
        if open.is_some() && open != wide {
            out.push_str("\\X0\\");
            open = None;
        }
        match wide {
            None => out.push(c),
            Some(width) => {
                if open.is_none() {
                    out.push_str(if width == 2 { "\\X2\\" } else { "\\X4\\" });
                    open = Some(width);
                }
                let _ = if width == 2 {
                    write!(out, "{:04X}", c as u32)
                } else {
                    write!(out, "{:08X}", c as u32)
                };
            }
        }
    }
    if open.is_some() {
        out.push_str("\\X0\\");
    }
    out
}

/// The text `encoded` encodes, read as ISO 10303-21 reads `\X2\` and `\X4\` (four or eight
/// hex digits a character, to `\X0\`), or none if it isn't [`step_encoded`]'s form: a
/// directive of another kind, a run unclosed or empty, hex not four or eight uppercase
/// digits, a surrogate or a value past U+10FFFF, or a character that isn't printable ASCII
/// outside a run.
pub fn step_decoded(encoded: &str) -> Option<String> {
    let mut out = String::with_capacity(encoded.len());
    let mut rest = encoded;
    while let Some(c) = rest.chars().next() {
        if c != '\\' {
            if !(' '..='~').contains(&c) {
                return None;
            }
            out.push(c);
            rest = &rest[1..];
            continue;
        }
        let digits = if rest.starts_with("\\X2\\") {
            4
        } else if rest.starts_with("\\X4\\") {
            8
        } else {
            return None;
        };
        rest = &rest[4..];
        let end = rest.find("\\X0\\")?;
        let run = &rest[..end];
        if run.is_empty() || !run.len().is_multiple_of(digits) {
            return None;
        }
        for group in run.as_bytes().chunks(digits) {
            if !group
                .iter()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b))
            {
                return None;
            }
            let value = u32::from_str_radix(std::str::from_utf8(group).ok()?, 16).ok()?;
            out.push(char::from_u32(value)?);
        }
        rest = &rest[end + 4..];
    }
    Some(out)
}

/// Whether `text` is in [`step_encoded`]'s form, exactly: the form the encoder gives the
/// text it decodes to, so no run is split, no character that needn't be is encoded, and no
/// character past U+FFFF is in a `\X2\` run. What the worker checks of every string it's
/// given to write.
pub fn is_step_encoded(text: &str) -> bool {
    step_decoded(text).is_some_and(|decoded| step_encoded(&decoded) == text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(parts: &[(&str, &str)]) -> ExportStep {
        ExportStep {
            parts: parts
                .iter()
                .map(|&(id, name)| StepPart {
                    shape: ShapeData::unchecked(Blob::inline(vec![1, 2, 3])),
                    id: id.to_owned(),
                    name: name.to_owned(),
                })
                .collect(),
            assembly: "bracket".to_owned(),
            file_name: "bracket.silico".to_owned(),
        }
    }

    #[test]
    fn an_export_of_encoded_strings_and_a_part_is_taken_and_any_other_refused_by_its_field() {
        assert_eq!(
            export(&[("f0000003", "Kl\\X2\\00F6\\X0\\tzchen")]).check(),
            Ok(())
        );
        assert_eq!(export(&[]).check().unwrap_err().parameter, "parts");
        assert_eq!(
            export(&[("f0000003", "a\\N\\b")])
                .check()
                .unwrap_err()
                .parameter,
            "name"
        );
        assert_eq!(
            export(&[("f\u{F6}", "a")]).check().unwrap_err().parameter,
            "id"
        );
        let mut e = export(&[("f0000003", "a")]);
        e.assembly = "Kl\u{F6}tzchen".to_owned();
        assert_eq!(e.check().unwrap_err().parameter, "assembly");
        let mut e = export(&[("f0000003", "a")]);
        e.file_name = "a\\b.silico".to_owned();
        assert_eq!(e.check().unwrap_err().parameter, "file_name");
        // The second part's name, after a first that's fine.
        assert_eq!(
            export(&[("f0000003", "a"), ("f0000004", "b\u{0}")])
                .check()
                .unwrap_err()
                .parameter,
            "name"
        );
    }

    #[test]
    fn printable_ascii_but_the_backslash_is_itself() {
        // The note's table: what OCCT passes through but for `'`, which it doubles.
        assert_eq!(step_encoded("Bracket 'q' (2) ~"), "Bracket 'q' (2) ~");
        assert_eq!(step_encoded(""), "");
    }

    #[test]
    fn the_notes_names_are_encoded_as_freecad_reads_them_back() {
        // docs/ports/step-export.md, *Strings*: each read back exactly by FreeCAD 1.1.3.
        assert_eq!(
            step_encoded("a\\N\\b"),
            "a\\X2\\005C\\X0\\N\\X2\\005C\\X0\\b"
        );
        assert_eq!(step_encoded("Klötzchen"), "Kl\\X2\\00F6\\X0\\tzchen");
        assert_eq!(
            step_encoded("smile \u{1F600} end"),
            "smile \\X4\\0001F600\\X0\\ end"
        );
        assert_eq!(
            step_encoded("C:\\X2\\temp"),
            "C:\\X2\\005C\\X0\\X2\\X2\\005C\\X0\\temp"
        );
    }

    #[test]
    fn runs_are_maximal_and_split_only_where_the_width_changes() {
        // Two characters of the BMP: one run. A tab and a line feed: one run. A BMP
        // character then an astral one: two runs, closed and opened back to back.
        assert_eq!(step_encoded("üö"), "\\X2\\00FC00F6\\X0\\");
        assert_eq!(step_encoded("a\t\nb"), "a\\X2\\0009000A\\X0\\b");
        assert_eq!(
            step_encoded("ü\u{1F600}\u{1F601}ö"),
            "\\X2\\00FC\\X0\\\\X4\\0001F6000001F601\\X0\\\\X2\\00F6\\X0\\"
        );
        // The last character below and above the BMP's edge.
        assert_eq!(step_encoded("\u{FFFF}"), "\\X2\\FFFF\\X0\\");
        assert_eq!(step_encoded("\u{10000}"), "\\X4\\00010000\\X0\\");
        assert_eq!(step_encoded("\u{10FFFF}"), "\\X4\\0010FFFF\\X0\\");
        // DEL and the C0 and C1 controls are encoded, as is U+0080.
        assert_eq!(
            step_encoded("\u{7F}\u{0}\u{9B}"),
            "\\X2\\007F0000009B\\X0\\"
        );
    }

    #[test]
    fn every_character_encodes_and_decodes_to_itself_and_its_form_is_checked() {
        // Every scalar value, a stride through them and each class's edges, alone and
        // between printable ASCII.
        let edges = [
            '\u{0}',
            '\u{1F}',
            ' ',
            '~',
            '\u{7F}',
            '\\',
            '\'',
            '\u{80}',
            '\u{D7FF}',
            '\u{E000}',
            '\u{FFFD}',
            '\u{FFFE}',
            '\u{FFFF}',
            '\u{10000}',
            '\u{10FFFF}',
        ];
        for c in (0u32..=0x10_FFFF)
            .step_by(251)
            .filter_map(char::from_u32)
            .chain(edges)
        {
            for text in [c.to_string(), format!("a{c}b{c}{c}")] {
                let encoded = step_encoded(&text);
                assert_eq!(
                    step_decoded(&encoded).as_deref(),
                    Some(text.as_str()),
                    "{c:?}"
                );
                assert!(is_step_encoded(&encoded), "{c:?}: {encoded}");
                assert!(
                    encoded.chars().all(|e| (' '..='~').contains(&e)),
                    "{c:?}: {encoded}"
                );
            }
        }
    }

    #[test]
    fn a_string_not_in_the_encoders_form_is_refused() {
        for bad in [
            // Raw past ASCII, a raw control, a raw backslash.
            "Kl\u{F6}tzchen",
            "a\tb",
            "a\\b",
            // Other directives OCCT would pass through.
            "a\\N\\b",
            "\\S\\a",
            "\\XFC\\",
            "\\PA\\",
            // Lowercase hex, a short group, an empty run, a run never closed.
            "\\X2\\00fc\\X0\\",
            "\\X2\\0FC\\X0\\",
            "\\X2\\\\X0\\",
            "\\X2\\00FC",
            "\\X4\\1F600\\X0\\",
            // A surrogate, a value past Unicode.
            "\\X2\\D800\\X0\\",
            "\\X4\\00110000\\X0\\",
            // Encoded, but not as the encoder would: a split run, printable ASCII encoded,
            // an astral character as a pair of surrogates' worth in X4's place in X2.
            "\\X2\\00FC\\X0\\\\X2\\00F6\\X0\\",
            "\\X2\\0041\\X0\\",
            "\\X4\\000000FC\\X0\\",
        ] {
            assert!(!is_step_encoded(bad), "{bad:?}");
        }
    }
}
