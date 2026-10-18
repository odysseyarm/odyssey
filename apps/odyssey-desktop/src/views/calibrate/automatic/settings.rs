//! Ranges for the values the wizard lets the user choose, shared by the setup
//! step, the sliders and the command line so they all accept the same inputs.

use std::ops::RangeInclusive;

/// Physical screen diagonals the wizard accepts. The solver would take any
/// positive number; the range rejects a typo such as 550 for 55.
pub const DIAGONAL_RANGE_IN: RangeInclusive<f64> = 10.0..=200.0;

/// Color exposure in microseconds. The camera clamps to what it supports; the
/// range covers the useful span from a bright screen in a dark room to a dim
/// one in daylight.
pub const COLOR_EXPOSURE_RANGE_US: RangeInclusive<u32> = 200..=10_000;
pub const COLOR_EXPOSURE_STEP_US: u32 = 100;

/// Infrared exposure for marker detection, microseconds. Short, so only the
/// marker LEDs stay bright.
pub const IR_EXPOSURE_RANGE_US: RangeInclusive<u32> = 20..=1_000;
pub const IR_EXPOSURE_STEP_US: u32 = 10;

/// Picture widths accepted for a projector, in centimeters.
pub const WIDTH_RANGE_CM: RangeInclusive<f64> = 20.0..=1000.0;

/// Which dimension the tech types in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeField {
    /// Corner to corner in inches, how screens are sold.
    Diagonal,
    /// Left to right in centimeters, how a projected picture is measured.
    Width,
}

/// A size the tech typed in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Entered {
    Diagonal { inches: f64 },
    Width { cm: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeError {
    NotANumber,
    OutOfRange(SizeField),
}

impl SizeError {
    pub fn message(self) -> String {
        match self {
            Self::NotANumber => "Enter the size as a number, like 55.".into(),
            Self::OutOfRange(SizeField::Diagonal) => format!(
                "Enter a size between {} and {} inches, or leave it blank.",
                DIAGONAL_RANGE_IN.start(),
                DIAGONAL_RANGE_IN.end()
            ),
            Self::OutOfRange(SizeField::Width) => format!(
                "Enter a width between {} and {} cm, or leave it blank.",
                WIDTH_RANGE_CM.start(),
                WIDTH_RANGE_CM.end()
            ),
        }
    }
}

/// Reads the optional size field. Blank means "measure it".
pub fn parse_entered(text: &str, field: SizeField) -> Result<Option<Entered>, SizeError> {
    let text = text.trim().trim_end_matches(['"', '″']).trim();
    let text = match field {
        SizeField::Diagonal => text,
        SizeField::Width => text.trim_end_matches("cm").trim(),
    };
    if text.is_empty() {
        return Ok(None);
    }
    // Accept a decimal comma, which is how many keyboards' locales type it.
    let value: f64 = text.replace(',', ".").parse().map_err(|_| SizeError::NotANumber)?;
    if !value.is_finite() {
        return Err(SizeError::NotANumber);
    }
    match field {
        SizeField::Diagonal if DIAGONAL_RANGE_IN.contains(&value) => Ok(Some(Entered::Diagonal { inches: value })),
        SizeField::Width if WIDTH_RANGE_CM.contains(&value) => Ok(Some(Entered::Width { cm: value })),
        _ => Err(SizeError::OutOfRange(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagonal(text: &str) -> Result<Option<f64>, SizeError> {
        parse_entered(text, SizeField::Diagonal).map(|e| {
            e.map(|e| match e {
                Entered::Diagonal { inches } => inches,
                Entered::Width { .. } => unreachable!(),
            })
        })
    }

    #[test]
    fn blank_means_measure() {
        assert_eq!(diagonal(""), Ok(None));
        assert_eq!(diagonal("   "), Ok(None));
        assert_eq!(parse_entered("", SizeField::Width), Ok(None));
    }

    #[test]
    fn accepts_inches_in_common_spellings() {
        assert_eq!(diagonal("55"), Ok(Some(55.0)));
        assert_eq!(diagonal(" 27.5 "), Ok(Some(27.5)));
        assert_eq!(diagonal("32,5"), Ok(Some(32.5)));
        assert_eq!(diagonal("65\""), Ok(Some(65.0)));
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(diagonal("big"), Err(SizeError::NotANumber));
        assert_eq!(diagonal("NaN"), Err(SizeError::NotANumber));
        assert_eq!(diagonal("inf"), Err(SizeError::NotANumber));
        assert_eq!(diagonal("550"), Err(SizeError::OutOfRange(SizeField::Diagonal)));
        assert_eq!(diagonal("-5"), Err(SizeError::OutOfRange(SizeField::Diagonal)));
    }

    #[test]
    fn widths_are_centimeters() {
        assert_eq!(parse_entered("240", SizeField::Width), Ok(Some(Entered::Width { cm: 240.0 })));
        assert_eq!(parse_entered("240 cm", SizeField::Width), Ok(Some(Entered::Width { cm: 240.0 })));
        assert_eq!(parse_entered("5", SizeField::Width), Err(SizeError::OutOfRange(SizeField::Width)));
        assert!(SizeError::OutOfRange(SizeField::Width).message().contains("cm"));
    }
}
