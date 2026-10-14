//! Lengths and angles as a document holds them: whole nanometres and nanodegrees in an
//! `i64` (ARCHITECTURE.md, Numbers in the file). They're read from a plain decimal's text
//! and written back as one, never through a float, so a document's numbers are exact and
//! the same on every machine. The kernel's `f64` comes from them at the boundary.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

/// Why a decimal's text isn't a value of a quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalError {
    /// Written with an exponent (`1e-3`): one value has one spelling, a plain decimal.
    Exponent,
    /// More precise than the quantity holds: a non-zero digit past its decimals.
    TooPrecise { decimals: u32 },
    /// Larger than an `i64` of the quantity's units holds.
    OutOfRange,
    /// Zero written with a minus sign.
    NegativeZero,
    /// Not a decimal at all (the reader checks JSON's number grammar first, so only a
    /// caller that skipped it sees this).
    Malformed,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecimalError::Exponent => f.write_str("is written with an exponent"),
            DecimalError::TooPrecise { decimals } => {
                write!(f, "has more than {decimals} decimals")
            }
            DecimalError::OutOfRange => f.write_str("is too large"),
            DecimalError::NegativeZero => f.write_str("is zero written with a minus sign"),
            DecimalError::Malformed => f.write_str("isn't a decimal"),
        }
    }
}

/// The value of `text`, a plain decimal, in units of 10^-`decimals`: `0.1` with 6 decimals
/// is 100,000. Digits past `decimals` must be zeros.
pub fn parse_decimal(text: &str, decimals: u32) -> Result<i64, DecimalError> {
    if text.contains(['e', 'E']) {
        return Err(DecimalError::Exponent);
    }
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (unsigned, ""),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (unsigned.contains('.') && !digits(fraction)) {
        return Err(DecimalError::Malformed);
    }
    let kept = fraction.get(..decimals as usize).unwrap_or(fraction);
    let dropped = fraction.get(decimals as usize..).unwrap_or("");
    if dropped.bytes().any(|b| b != b'0') {
        return Err(DecimalError::TooPrecise { decimals });
    }
    // The magnitude, in i128 so that i64::MIN's is held; checked at every step, so a
    // thousand-digit number stops at the first digit that overflows.
    let mut magnitude: i128 = 0;
    let push = |m: i128, b: u8| {
        m.checked_mul(10)
            .and_then(|m| m.checked_add(i128::from(b - b'0')))
            .filter(|&m| m <= i128::from(i64::MAX) + 1)
            .ok_or(DecimalError::OutOfRange)
    };
    for b in whole.bytes().chain(kept.bytes()) {
        magnitude = push(magnitude, b)?;
    }
    for _ in kept.len()..decimals as usize {
        magnitude = push(magnitude, b'0')?;
    }
    let value = if negative { -magnitude } else { magnitude };
    if negative && magnitude == 0 {
        return Err(DecimalError::NegativeZero);
    }
    i64::try_from(value).map_err(|_| DecimalError::OutOfRange)
}

/// `value`, in units of 10^-`decimals`, as the shortest plain decimal that reads back as it:
/// `100000` with 6 decimals is `0.1`, `10000000` is `10`.
pub fn write_decimal(value: i64, decimals: u32) -> String {
    let scale = 10u64.pow(decimals);
    let magnitude = value.unsigned_abs();
    let (whole, fraction) = (magnitude / scale, magnitude % scale);
    let sign = if value < 0 { "-" } else { "" };
    if fraction == 0 {
        return format!("{sign}{whole}");
    }
    let digits = format!("{fraction:0width$}", width = decimals as usize);
    format!("{sign}{whole}.{}", digits.trim_end_matches('0'))
}

/// A length: whole nanometres. Written in millimetres, to 6 decimals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Length(i64);

/// A length's decimals in millimetres: nanometres.
pub const LENGTH_DECIMALS: u32 = 6;

impl Length {
    pub const fn from_nm(nm: i64) -> Self {
        Length(nm)
    }

    pub const fn nm(self) -> i64 {
        self.0
    }

    /// The length in millimetres, as the kernel takes it: one correctly rounded division,
    /// the same on every platform, exact in its whole millimetres to 2^53 nm.
    pub fn mm(self) -> f64 {
        self.0 as f64 / 1e6
    }

    /// The length a decimal's text (in millimetres) writes.
    pub fn parse(text: &str) -> Result<Self, DecimalError> {
        parse_decimal(text, LENGTH_DECIMALS).map(Length)
    }
}

impl fmt::Display for Length {
    /// In millimetres, as the file writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_decimal(self.0, LENGTH_DECIMALS))
    }
}

impl JsonSchema for Length {
    fn schema_name() -> Cow<'static, str> {
        "Length".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A length in millimetres, written as a plain decimal (no exponent), to at most 6 decimals (nanometres).",
            "type": "number",
            "multipleOf": 0.000001
        })
    }
}

/// An angle: whole nanodegrees. Written in degrees, to 9 decimals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Angle(i64);

/// An angle's decimals in degrees: nanodegrees.
pub const ANGLE_DECIMALS: u32 = 9;

impl Angle {
    pub const fn from_ndeg(ndeg: i64) -> Self {
        Angle(ndeg)
    }

    pub const fn ndeg(self) -> i64 {
        self.0
    }

    /// The angle in degrees, as the kernel takes it (one correctly rounded division).
    pub fn degrees(self) -> f64 {
        self.0 as f64 / 1e9
    }

    /// The angle a decimal's text (in degrees) writes.
    pub fn parse(text: &str) -> Result<Self, DecimalError> {
        parse_decimal(text, ANGLE_DECIMALS).map(Angle)
    }
}

impl fmt::Display for Angle {
    /// In degrees, as the file writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_decimal(self.0, ANGLE_DECIMALS))
    }
}

impl JsonSchema for Angle {
    fn schema_name() -> Cow<'static, str> {
        "Angle".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An angle in degrees, written as a plain decimal (no exponent), to at most 9 decimals (nanodegrees).",
            "type": "number",
            "multipleOf": 0.000000001
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_read_exactly_without_a_float() {
        // 0.1 is exactly 100,000 nm; no f64 holds 0.1 exactly.
        assert_eq!(Length::parse("0.1"), Ok(Length::from_nm(100_000)));
        assert_eq!(Length::parse("10"), Ok(Length::from_nm(10_000_000)));
        assert_eq!(Length::parse("-2.5"), Ok(Length::from_nm(-2_500_000)));
        assert_eq!(Length::parse("0.000001"), Ok(Length::from_nm(1)));
        assert_eq!(Length::parse("0"), Ok(Length::from_nm(0)));
        assert_eq!(Length::parse("0.0"), Ok(Length::from_nm(0)));
        // Zeros past the resolution add no precision.
        assert_eq!(Length::parse("10.50"), Ok(Length::from_nm(10_500_000)));
        assert_eq!(
            Length::parse("1.0000000000"),
            Ok(Length::from_nm(1_000_000))
        );
        assert_eq!(Angle::parse("0.000000001"), Ok(Angle::from_ndeg(1)));
        assert_eq!(Angle::parse("90"), Ok(Angle::from_ndeg(90_000_000_000)));
    }

    #[test]
    fn decimals_that_dont_fit_are_refused_saying_why() {
        assert_eq!(
            Length::parse("0.0000001"),
            Err(DecimalError::TooPrecise { decimals: 6 })
        );
        assert_eq!(
            Length::parse("1.0000005"),
            Err(DecimalError::TooPrecise { decimals: 6 })
        );
        assert_eq!(
            Angle::parse("0.0000000001"),
            Err(DecimalError::TooPrecise { decimals: 9 })
        );
        assert_eq!(Length::parse("1e-3"), Err(DecimalError::Exponent));
        assert_eq!(Length::parse("1E3"), Err(DecimalError::Exponent));
        assert_eq!(Length::parse("-0"), Err(DecimalError::NegativeZero));
        assert_eq!(Length::parse("-0.000"), Err(DecimalError::NegativeZero));
        // i64 nanometres: ±9,223,372,036,854.775807 mm.
        assert_eq!(
            Length::parse("9223372036854.775807"),
            Ok(Length::from_nm(i64::MAX))
        );
        assert_eq!(
            Length::parse("-9223372036854.775808"),
            Ok(Length::from_nm(i64::MIN))
        );
        assert_eq!(
            Length::parse("9223372036854.775808"),
            Err(DecimalError::OutOfRange)
        );
        assert_eq!(
            Length::parse("-9223372036854.775809"),
            Err(DecimalError::OutOfRange)
        );
        assert_eq!(
            Length::parse(&"9".repeat(10_000)),
            Err(DecimalError::OutOfRange)
        );
        for bad in ["", "-", ".5", "5.", "+5", "1.2.3", "0x10", " 1"] {
            assert_eq!(Length::parse(bad), Err(DecimalError::Malformed), "{bad:?}");
        }
    }

    #[test]
    fn values_write_as_the_shortest_decimal_that_reads_back() {
        let cases = [
            (100_000, "0.1"),
            (10_000_000, "10"),
            (10_500_000, "10.5"),
            (-2_500_000, "-2.5"),
            (1, "0.000001"),
            (-1, "-0.000001"),
            (0, "0"),
            (i64::MAX, "9223372036854.775807"),
            (i64::MIN, "-9223372036854.775808"),
        ];
        for (nm, text) in cases {
            assert_eq!(Length::from_nm(nm).to_string(), text);
            assert_eq!(Length::parse(text), Ok(Length::from_nm(nm)));
        }
        assert_eq!(Angle::from_ndeg(45_000_000_001).to_string(), "45.000000001");
        assert_eq!(
            Angle::from_ndeg(i64::MIN).to_string(),
            "-9223372036.854775808"
        );
    }

    #[test]
    fn the_kernels_float_is_the_one_nearest_the_decimal() {
        // One correctly rounded division gives the f64 nearest the exact value.
        assert_eq!(Length::parse("0.1").unwrap().mm(), 0.1);
        assert_eq!(Length::parse("10.5").unwrap().mm(), 10.5);
        assert_eq!(Length::parse("0.000001").unwrap().mm(), 1e-6);
        assert_eq!(Length::parse("1000000").unwrap().mm(), 1e6);
        assert_eq!(Angle::parse("0.1").unwrap().degrees(), 0.1);
        assert_eq!(Angle::parse("360").unwrap().degrees(), 360.0);
    }
}
