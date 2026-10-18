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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagonalError {
    NotANumber,
    OutOfRange,
}

impl DiagonalError {
    pub fn message(self) -> String {
        match self {
            Self::NotANumber => "Enter the size as a number, like 55.".into(),
            Self::OutOfRange => format!(
                "Enter a size between {} and {} inches, or leave it blank.",
                DIAGONAL_RANGE_IN.start(),
                DIAGONAL_RANGE_IN.end()
            ),
        }
    }
}

/// Reads the optional diagonal field. Blank means "measure it".
pub fn parse_diagonal(text: &str) -> Result<Option<f64>, DiagonalError> {
    let text = text.trim().trim_end_matches(['"', '″']).trim();
    if text.is_empty() {
        return Ok(None);
    }
    // Accept a decimal comma, which is how many keyboards' locales type it.
    let value: f64 = text.replace(',', ".").parse().map_err(|_| DiagonalError::NotANumber)?;
    if !value.is_finite() {
        return Err(DiagonalError::NotANumber);
    }
    if !DIAGONAL_RANGE_IN.contains(&value) {
        return Err(DiagonalError::OutOfRange);
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_means_measure() {
        assert_eq!(parse_diagonal(""), Ok(None));
        assert_eq!(parse_diagonal("   "), Ok(None));
    }

    #[test]
    fn accepts_inches_in_common_spellings() {
        assert_eq!(parse_diagonal("55"), Ok(Some(55.0)));
        assert_eq!(parse_diagonal(" 27.5 "), Ok(Some(27.5)));
        assert_eq!(parse_diagonal("32,5"), Ok(Some(32.5)));
        assert_eq!(parse_diagonal("65\""), Ok(Some(65.0)));
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(parse_diagonal("big"), Err(DiagonalError::NotANumber));
        assert_eq!(parse_diagonal("NaN"), Err(DiagonalError::NotANumber));
        assert_eq!(parse_diagonal("inf"), Err(DiagonalError::NotANumber));
        assert_eq!(parse_diagonal("550"), Err(DiagonalError::OutOfRange));
        assert_eq!(parse_diagonal("-5"), Err(DiagonalError::OutOfRange));
    }
}
