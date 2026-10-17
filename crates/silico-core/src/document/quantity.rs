//! Lengths and angles as a document holds them: whole nanometres and nanodegrees in an
//! `i64` (ARCHITECTURE.md, Numbers in the file). They're read from a number's text, with
//! or without an exponent, and written back as a plain decimal, never through a float, so
//! a document's numbers are exact and the same on every machine. The kernel's `f64` comes
//! from them at the boundary.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

/// Why a number's text isn't a value of a quantity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberError {
    /// More precise than the quantity holds: a non-zero digit past its decimals.
    TooPrecise { decimals: u32 },
    /// Larger, either way, than the quantity holds.
    OutOfRange,
    /// Zero written with a minus sign.
    NegativeZero,
    /// Not a number at all (the reader checks JSON's number grammar first, so only a
    /// caller that skipped it sees this).
    Malformed,
}

impl fmt::Display for NumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberError::TooPrecise { decimals } => {
                write!(f, "has more than {decimals} decimals")
            }
            NumberError::OutOfRange => f.write_str("is too large"),
            NumberError::NegativeZero => f.write_str("is zero written with a minus sign"),
            NumberError::Malformed => f.write_str("isn't a number"),
        }
    }
}

/// The largest exponent read as written; past it, the value is out of range (or more
/// precise than any quantity) whatever its digits, so it's held there.
const EXPONENT_CAP: i128 = 1 << 40;

/// The value of `text`, a JSON number (a decimal, with or without an exponent: `0.1`,
/// `1e-05`, `1.05E1`), in units of 10^-`decimals`: `0.1` with 6 decimals is 100,000. Read
/// from its digits, never through a float. Digits past `decimals` must be zeros.
pub fn parse_number(text: &str, decimals: u32) -> Result<i64, NumberError> {
    parse_wide(text, decimals)
        .and_then(|value| i64::try_from(value).map_err(|_| NumberError::OutOfRange))
}

/// As [`parse_number`], to an `i128`: as large as one holds, either way.
pub fn parse_wide(text: &str, decimals: u32) -> Result<i128, NumberError> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (
            unsigned.get(..at).unwrap_or(""),
            Some(unsigned.get(at + 1..).unwrap_or("")),
        ),
        None => (unsigned, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (mantissa, ""),
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (mantissa.contains('.') && !digits(fraction)) {
        return Err(NumberError::Malformed);
    }
    let exponent = match exponent {
        None => 0,
        Some(e) => {
            let (sign, magnitude) = match e.strip_prefix('-') {
                Some(rest) => (-1, rest),
                None => (1, e.strip_prefix('+').unwrap_or(e)),
            };
            if !digits(magnitude) {
                return Err(NumberError::Malformed);
            }
            let held = magnitude.bytes().fold(0i128, |m, b| {
                (m * 10 + i128::from(b - b'0')).min(EXPONENT_CAP)
            });
            sign * held
        }
    };
    // The value is the mantissa's digits times 10^(exponent - fraction digits); in units of
    // 10^-decimals, the digits times 10^shift.
    let shift = exponent - fraction.len() as i128 + i128::from(decimals);
    let mut significant: Vec<u8> = whole
        .bytes()
        .chain(fraction.bytes())
        .skip_while(|&b| b == b'0')
        .collect();
    if significant.is_empty() {
        return if negative {
            Err(NumberError::NegativeZero)
        } else {
            Ok(0)
        };
    }
    if shift < 0 {
        // Digits past the resolution: dropped if they're zeros, refused if not.
        let cut = usize::try_from(-shift).unwrap_or(usize::MAX);
        if cut >= significant.len() {
            return Err(NumberError::TooPrecise { decimals });
        }
        let kept = significant.len() - cut;
        if significant
            .get(kept..)
            .is_some_and(|d| d.iter().any(|&b| b != b'0'))
        {
            return Err(NumberError::TooPrecise { decimals });
        }
        significant.truncate(kept);
    }
    // The magnitude, checked at every step, so a thousand-digit number stops at the first
    // digit that overflows.
    let push = |m: i128, b: u8| {
        m.checked_mul(10)
            .and_then(|m| m.checked_add(i128::from(b - b'0')))
            .ok_or(NumberError::OutOfRange)
    };
    let mut magnitude: i128 = 0;
    for &b in &significant {
        magnitude = push(magnitude, b)?;
    }
    // Zeros after: a non-zero value with 40 of them is past i128, so `push` refuses it by
    // then, however large the shift.
    for _ in 0..shift.clamp(0, 40) {
        magnitude = push(magnitude, b'0')?;
    }
    Ok(if negative { -magnitude } else { magnitude })
}

/// `value` in units of 10^-`decimals`, rounded to the nearest, ties to even, from its exact
/// value (Rust writes a float to a number of decimals exactly); none if it isn't finite or
/// doesn't fit an `i128`. A value that rounds to zero is zero, whatever its sign.
pub fn round_to(value: f64, decimals: u32) -> Option<i128> {
    if !value.is_finite() {
        return None;
    }
    let text = format!("{value:.*}", decimals as usize);
    match parse_wide(&text, decimals) {
        Ok(units) => Some(units),
        Err(NumberError::NegativeZero) => Some(0),
        Err(_) => None,
    }
}

/// `value`, in units of 10^-`decimals`, as the shortest plain decimal that reads back as it:
/// `100000` with 6 decimals is `0.1`, `10000000` is `10`.
pub fn write_decimal(value: i128, decimals: u32) -> String {
    let scale = 10u128.pow(decimals);
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

    /// The length a number's text (in millimetres) writes.
    pub fn parse(text: &str) -> Result<Self, NumberError> {
        parse_number(text, LENGTH_DECIMALS).map(Length)
    }

    /// `mm` millimetres, rounded to the nanometre ([`round_to`]); none if it isn't finite
    /// or doesn't fit.
    pub fn rounded(mm: f64) -> Option<Self> {
        i64::try_from(round_to(mm, LENGTH_DECIMALS)?)
            .ok()
            .map(Length)
    }
}

impl fmt::Display for Length {
    /// In millimetres, as the file writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_decimal(i128::from(self.0), LENGTH_DECIMALS))
    }
}

impl JsonSchema for Length {
    fn schema_name() -> Cow<'static, str> {
        "Length".into()
    }

    /// No `multipleOf` for the resolution: validators divide in floating point, so
    /// `0.1 / 1e-6` isn't whole to them and they'd refuse 0.1 mm. The reader is the
    /// authority on decimals.
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A length in millimetres: a number, to at most 6 decimals (nanometres). An exponent is read exactly; Silico writes plain decimals. (Strings are reserved for expressions, which aren't read yet.)",
            "type": "number"
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

    /// The angle a number's text (in degrees) writes.
    pub fn parse(text: &str) -> Result<Self, NumberError> {
        parse_number(text, ANGLE_DECIMALS).map(Angle)
    }
}

impl fmt::Display for Angle {
    /// In degrees, as the file writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&write_decimal(i128::from(self.0), ANGLE_DECIMALS))
    }
}

impl JsonSchema for Angle {
    fn schema_name() -> Cow<'static, str> {
        "Angle".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "An angle in degrees: a number, to at most 9 decimals (nanodegrees). An exponent is read exactly; Silico writes plain decimals. (Strings are reserved for expressions, which aren't read yet.)",
            "type": "number"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_rounded_past_i64_nanometres_is_none() {
        // i64 holds up to 9223372036854775807 nm, about 9.2 million km.
        assert_eq!(
            Length::rounded(9e12),
            Some(Length::from_nm(9_000_000_000_000_000_000))
        );
        assert_eq!(
            Length::rounded(-9e12).map(Length::nm),
            Some(-9_000_000_000_000_000_000)
        );
        assert_eq!(Length::rounded(1e13), None);
        // i64::MAX nm is 9223372036854.775807 mm; the floats either side of it, 2^-9 apart
        // (9223372036854.775390625 and 9223372036854.77734375, written as Rust's shortest).
        assert_eq!(
            Length::rounded(9_223_372_036_854.775),
            Some(Length::from_nm(9_223_372_036_854_775_391))
        );
        assert_eq!(Length::rounded(9_223_372_036_854.777), None);
        assert_eq!(Length::rounded(9.5e12), None);
        assert_eq!(Length::rounded(-1e13), None);
        assert_eq!(Length::rounded(f64::NAN), None);
        assert_eq!(Length::rounded(0.0000004), Some(Length::from_nm(0)));
        // The float written 1.0000005 is 1.00000050000000006988..., past the half: up.
        assert_eq!(Length::rounded(1.0000005), Some(Length::from_nm(1_000_001)));
    }

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
    fn exponents_read_exactly_as_the_decimal_they_write() {
        // As Python writes small floats (`1e-05`) and JavaScript large ones: the same
        // values as their plain decimals, read from the digits.
        let cases = [
            ("1e-05", 10),
            ("1E-5", 10),
            ("1e-6", 1),
            ("1.5e-5", 15),
            ("1.05e1", 10_500_000),
            ("1.05E+1", 10_500_000),
            ("25e-1", 2_500_000),
            ("0.001e3", 1_000_000),
            ("123456.789012e0", 123_456_789_012),
            ("1e6", 1_000_000_000_000),
            ("-2.5e-3", -2_500),
            ("10000000e-13", 1),
            ("0e999999999", 0),
            ("0.000e-5", 0),
            ("9.223372036854775807e12", i64::MAX),
            ("-9.223372036854775808e12", i64::MIN),
        ];
        for (text, nm) in cases {
            assert_eq!(Length::parse(text), Ok(Length::from_nm(nm)), "{text}");
        }
        assert_eq!(Angle::parse("4.5e1"), Ok(Angle::from_ndeg(45_000_000_000)));
        assert_eq!(Angle::parse("1e-9"), Ok(Angle::from_ndeg(1)));
        assert_eq!(
            Length::parse("9.223372036854775808e12"),
            Err(NumberError::OutOfRange)
        );
    }

    #[test]
    fn an_exponent_past_i128_is_held_not_overflowed() {
        // 40 digits of exponent: past i128 unless held at the cap.
        let big = format!("1e{}", "9".repeat(40));
        assert_eq!(Length::parse(&big), Err(NumberError::OutOfRange));
        let small = format!("1e-{}", "9".repeat(40));
        assert_eq!(
            Length::parse(&small),
            Err(NumberError::TooPrecise { decimals: 6 })
        );
        // A mantissa as long as a document may be, brought back by its exponent: exact (a
        // cap below the document's size would refuse it).
        let n = 15_000_000;
        let text = format!("1{}e-{n}", "0".repeat(n));
        assert_eq!(Length::parse(&text), Ok(Length::from_nm(1_000_000)));
    }

    #[test]
    fn decimals_that_dont_fit_are_refused_saying_why() {
        assert_eq!(
            Length::parse("0.0000001"),
            Err(NumberError::TooPrecise { decimals: 6 })
        );
        assert_eq!(
            Length::parse("1.0000005"),
            Err(NumberError::TooPrecise { decimals: 6 })
        );
        assert_eq!(
            Angle::parse("0.0000000001"),
            Err(NumberError::TooPrecise { decimals: 9 })
        );
        // An exponent past the resolution, or past i64, the same.
        assert_eq!(
            Length::parse("1e-7"),
            Err(NumberError::TooPrecise { decimals: 6 })
        );
        assert_eq!(
            Length::parse("1.5e-6"),
            Err(NumberError::TooPrecise { decimals: 6 })
        );
        assert_eq!(
            Length::parse("1e-99999999999999999999"),
            Err(NumberError::TooPrecise { decimals: 6 })
        );
        assert_eq!(Length::parse("1e13"), Err(NumberError::OutOfRange));
        assert_eq!(
            Length::parse("1e99999999999999999999"),
            Err(NumberError::OutOfRange)
        );
        assert_eq!(Length::parse("-0e5"), Err(NumberError::NegativeZero));
        assert_eq!(Length::parse("-0.0E-3"), Err(NumberError::NegativeZero));
        assert_eq!(Length::parse("-0"), Err(NumberError::NegativeZero));
        assert_eq!(Length::parse("-0.000"), Err(NumberError::NegativeZero));
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
            Err(NumberError::OutOfRange)
        );
        assert_eq!(
            Length::parse("-9223372036854.775809"),
            Err(NumberError::OutOfRange)
        );
        assert_eq!(
            Length::parse(&"9".repeat(10_000)),
            Err(NumberError::OutOfRange)
        );
        for bad in [
            "", "-", ".5", "5.", "+5", "1.2.3", "0x10", " 1", "1e", "1e+", "e5", "1e1.5",
        ] {
            assert_eq!(Length::parse(bad), Err(NumberError::Malformed), "{bad:?}");
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
