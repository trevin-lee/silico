//! A feature's fingerprint (ARCHITECTURE.md, Fingerprints): what its solid measures, kept in
//! the document with it, as a lockfile keeps what a dependency resolved to. A rebuild that
//! measures otherwise is drift: a determinism bug or a kernel change, for a person to look
//! at. Its numbers are rounded to 6 decimals (the box to whole nanometres, the document's
//! resolution), to the nearest, ties to even, from the measure's exact value; so a diff reads
//! `8000`, not `7999.999999999998`, and the same measure always writes the same text.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use silico_protocol::ShapeSummary;

use super::quantity::{Length, NumberError, parse_wide, round_to, write_decimal};

/// A volume (mm³) or area (mm²), to 6 decimals: millionths in an `i128`, so any solid the
/// validation gate admits (a kilometre each way) is measured, with room to spare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Measure(i128);

/// A measure's decimals.
pub const MEASURE_DECIMALS: u32 = 6;

impl Measure {
    /// The measure in millionths of its unit.
    pub const fn from_millionths(millionths: i128) -> Self {
        Measure(millionths)
    }

    pub const fn millionths(self) -> i128 {
        self.0
    }

    /// The measure a number's text writes.
    pub fn parse(text: &str) -> Result<Self, NumberError> {
        parse_wide(text, MEASURE_DECIMALS).map(Measure)
    }

    /// `value` rounded to 6 decimals; none if it isn't finite, or is too large to hold.
    pub fn rounded(value: f64) -> Option<Self> {
        round_to(value, MEASURE_DECIMALS).map(Measure)
    }
}

impl fmt::Display for Measure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_decimal(self.0, MEASURE_DECIMALS))
    }
}

impl JsonSchema for Measure {
    fn schema_name() -> Cow<'static, str> {
        "Measure".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A volume (mm³) or area (mm²): a number, to at most 6 decimals.",
            "type": "number"
        })
    }
}

/// What a feature's solid measures: its volume and area, its tight bounding box (lowest x, y,
/// z, then highest; none for a solid with nothing in it), and how many faces, edges, and
/// vertices it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub volume: Measure,
    pub area: Measure,
    /// `[x, y, z, x, y, z]`: the lowest corner, then the highest (left out when the solid
    /// has nothing in it).
    #[serde(default)]
    #[schemars(with = "[Length; 6]")]
    pub bbox: Option<[Length; 6]>,
    #[schemars(range(max = 4_294_967_295u32))]
    pub faces: u32,
    #[schemars(range(max = 4_294_967_295u32))]
    pub edges: u32,
    #[schemars(range(max = 4_294_967_295u32))]
    pub vertices: u32,
}

impl Fingerprint {
    /// The fingerprint of a solid with `summary`; none if a measure isn't finite, or past
    /// what it can hold (a box corner past i64 nanometres, about 9.2 million km, or a volume
    /// past i128 millionths of a mm³, about 1.7e32 mm³: far past anything the validation
    /// gate admits).
    pub fn of(summary: &ShapeSummary) -> Option<Self> {
        let bbox = match summary.bounds {
            None => None,
            Some([low, high]) => {
                let mut corners = [Length::default(); 6];
                for (corner, value) in corners.iter_mut().zip(low.into_iter().chain(high)) {
                    *corner = Length::rounded(value)?;
                }
                Some(corners)
            }
        };
        Some(Fingerprint {
            volume: Measure::rounded(summary.volume)?,
            area: Measure::rounded(summary.area)?,
            bbox,
            faces: summary.faces,
            edges: summary.edges,
            vertices: summary.vertices,
        })
    }

    /// How `rebuilt` differs from this one, field by field, in the file's order: each field's
    /// name, as stored, and as rebuilt (in the file's spelling).
    pub fn differences(&self, rebuilt: &Fingerprint) -> Vec<Difference> {
        let mut differences = Vec::new();
        let mut compare = |field: &'static str, stored: String, now: String| {
            if stored != now {
                differences.push(Difference {
                    field,
                    stored,
                    rebuilt: now,
                });
            }
        };
        compare(
            "volume",
            self.volume.to_string(),
            rebuilt.volume.to_string(),
        );
        compare("area", self.area.to_string(), rebuilt.area.to_string());
        compare("bbox", bbox_text(&self.bbox), bbox_text(&rebuilt.bbox));
        compare("faces", self.faces.to_string(), rebuilt.faces.to_string());
        compare("edges", self.edges.to_string(), rebuilt.edges.to_string());
        compare(
            "vertices",
            self.vertices.to_string(),
            rebuilt.vertices.to_string(),
        );
        differences
    }
}

/// A field of a fingerprint that a rebuild measured otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub field: &'static str,
    pub stored: String,
    pub rebuilt: String,
}

/// A box as the file writes it, or `none`.
fn bbox_text(bbox: &Option<[Length; 6]>) -> String {
    match bbox {
        Some(corners) => format!(
            "[{}]",
            corners
                .iter()
                .map(Length::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => "none".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_round_to_6_decimals_from_their_exact_value() {
        // 7999.999999999998 (an f64 a hair under 8000) writes 8000.
        assert_eq!(
            Measure::rounded(7999.999999999998),
            Some(Measure(8_000_000_000))
        );
        assert_eq!(Measure::rounded(8000.0), Some(Measure(8_000_000_000)));
        // π r² h for r 2.5, h 12: 235.61944901923448 → 235.619449.
        assert_eq!(
            Measure::rounded(235.619_449_019_234_48),
            Some(Measure(235_619_449))
        );
        // Ties to even, on values an f64 holds exactly: 1/128 = 0.0078125, 3/128 = 0.0234375.
        assert_eq!(Measure::rounded(0.0078125), Some(Measure(7_812)));
        assert_eq!(Measure::rounded(0.0234375), Some(Measure(23_438)));
        // The nearest f64 to 0.0000005 is a little under it (4.99999999999999977e-7): down.
        assert_eq!(Measure::rounded(0.0000005), Some(Measure(0)));
        // A hair under zero is zero, unsigned.
        assert_eq!(Measure::rounded(-1e-12), Some(Measure(0)));
        assert_eq!(Measure::rounded(-0.0), Some(Measure(0)));
        assert_eq!(Measure::rounded(-2.5), Some(Measure(-2_500_000)));
    }

    #[test]
    fn a_measure_that_isnt_finite_or_doesnt_fit_has_none() {
        assert_eq!(Measure::rounded(f64::NAN), None);
        assert_eq!(Measure::rounded(f64::INFINITY), None);
        assert_eq!(Measure::rounded(f64::NEG_INFINITY), None);
        // i128::MAX millionths is about 1.7014e32: 1e32 (exactly
        // 100000000000000005366162204393472) fits, 1.7e32 too, 2e32 doesn't.
        assert_eq!(
            Measure::rounded(1e32),
            Some(Measure(100_000_000_000_000_005_366_162_204_393_472_000_000))
        );
        assert!(Measure::rounded(1.7e32).is_some());
        assert_eq!(Measure::rounded(2e32), None);
        assert_eq!(Measure::rounded(-2e32), None);
        // Past i64 now: a 2 km cube's volume, 8e18 mm³, and its millionths.
        assert_eq!(
            Measure::rounded(8e18),
            Some(Measure(8_000_000_000_000_000_000_000_000))
        );
    }

    fn summary() -> ShapeSummary {
        ShapeSummary {
            volume: 7999.999999999998,
            area: 2800.0000000000005,
            center: [20.0, 10.0, 5.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[-1e-12, 0.0, 0.0], [40.0, 20.0000000001, 10.0]]),
            valid: true,
        }
    }

    #[test]
    fn a_fingerprint_is_its_summarys_measures_rounded() {
        let fingerprint = Fingerprint::of(&summary()).unwrap();
        assert_eq!(fingerprint.volume.to_string(), "8000");
        assert_eq!(fingerprint.area.to_string(), "2800");
        assert_eq!(bbox_text(&fingerprint.bbox), "[0, 0, 0, 40, 20, 10]");
        assert_eq!(
            (fingerprint.faces, fingerprint.edges, fingerprint.vertices),
            (6, 12, 8)
        );
        let empty = ShapeSummary {
            bounds: None,
            ..summary()
        };
        assert_eq!(Fingerprint::of(&empty).unwrap().bbox, None);
        let unbounded = ShapeSummary {
            bounds: Some([[0.0; 3], [f64::INFINITY, 1.0, 1.0]]),
            ..summary()
        };
        assert_eq!(Fingerprint::of(&unbounded), None);
    }

    #[test]
    fn differences_name_each_field_that_moved_as_the_file_writes_it() {
        let stored = Fingerprint::of(&summary()).unwrap();
        assert!(stored.differences(&stored).is_empty());
        let rebuilt = Fingerprint {
            volume: Measure(7_999_500_000),
            faces: 7,
            bbox: None,
            ..stored
        };
        let fields: Vec<_> = stored
            .differences(&rebuilt)
            .into_iter()
            .map(|d| (d.field, d.stored, d.rebuilt))
            .collect();
        assert_eq!(
            fields,
            [
                ("volume", "8000".to_owned(), "7999.5".to_owned()),
                (
                    "bbox",
                    "[0, 0, 0, 40, 20, 10]".to_owned(),
                    "none".to_owned()
                ),
                ("faces", "6".to_owned(), "7".to_owned()),
            ]
        );
    }
}
