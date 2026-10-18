//! Sizing a picture by hand: a tape measure on a line the screen draws, or a
//! credit card held against the glass.
//!
//! Both come down to the same number, millimeters per pixel of the picture as
//! it is actually displayed. Overscan and scaling are already in it, which is
//! why a size taken from it needs no edge rule. The size of the whole picture
//! is then that pitch times the resolution. Everything here is plain functions
//! so the numbers can be tested without a window.

use std::ops::RangeInclusive;

use odyssey_autocalib::session::KnownSize;

/// An ISO/IEC 7810 ID-1 card: bank cards, driver's licences and most ID
/// cards, 85.60 by 53.98 mm.
pub const CARD_WIDTH_MM: f64 = 85.60;
pub const CARD_HEIGHT_MM: f64 = 53.98;
/// The corner radius of an ID-1 card, 3.18 mm, as a share of its width.
pub const CARD_CORNER_OF_WIDTH: f64 = 3.18 / CARD_WIDTH_MM;

/// How far a card is nudged per click, in screen pixels.
pub const CARD_STEP_PX: f64 = 0.25;

/// Pixel pitches the helper accepts, millimeters per pixel: a phone is about
/// 0.05, a wall-sized projected picture a few tenths up to about a millimeter.
/// Anything outside is a typo or the wrong unit.
pub const MM_PER_PX_RANGE: RangeInclusive<f64> = 0.02..=3.0;

/// Where the card starts when nothing says how large the pixels are: a flat
/// screen at typical TV density, or a picture thrown on a wall.
const DEFAULT_FLAT_MM_PER_PX: f64 = 0.25;
const DEFAULT_PROJECTED_MM_PER_PX: f64 = 1.0;

/// How the tech measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// A tape measure on a line the screen draws.
    Tape,
    /// A credit card held against the glass.
    Card,
}

/// A length in the tech's own unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Inches,
    Centimeters,
    Millimeters,
}

impl Unit {
    fn mm(self) -> f64 {
        match self {
            Self::Inches => 25.4,
            Self::Centimeters => 10.0,
            Self::Millimeters => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceError {
    NotALength,
    Implausible,
}

impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotALength => "Enter the length as a number, like 47.5 or 47 5/8.",
            Self::Implausible => {
                "That length doesn't fit the line on the screen. Check the unit, and measure from the middle of one tick to the middle of the other."
            }
        })
    }
}

impl std::error::Error for ReferenceError {}

/// The size of one pixel of the picture, as measured by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reference {
    pub method: Method,
    pub mm_per_px: f64,
}

impl Reference {
    /// From a tape measure laid along the line the screen draws: `line_px`
    /// pixels between the middles of its two ticks, `length_mm` on the tape.
    pub fn from_tape(line_px: f64, length_mm: f64) -> Result<Self, ReferenceError> {
        if !(line_px.is_finite() && line_px > 0.0 && length_mm.is_finite() && length_mm > 0.0) {
            return Err(ReferenceError::NotALength);
        }
        Self::checked(Method::Tape, length_mm / line_px)
    }

    /// From a card matched against a frame `card_px` pixels wide.
    pub fn from_card(card_px: f64) -> Result<Self, ReferenceError> {
        if !(card_px.is_finite() && card_px > 0.0) {
            return Err(ReferenceError::NotALength);
        }
        Self::checked(Method::Card, CARD_WIDTH_MM / card_px)
    }

    fn checked(method: Method, mm_per_px: f64) -> Result<Self, ReferenceError> {
        MM_PER_PX_RANGE
            .contains(&mm_per_px)
            .then_some(Self { method, mm_per_px })
            .ok_or(ReferenceError::Implausible)
    }

    /// Width and height in meters of a picture of `width_px` by `height_px`.
    pub fn size_m(&self, width_px: u32, height_px: u32) -> [f64; 2] {
        let m_per_px = self.mm_per_px / 1000.0;
        [
            f64::from(width_px) * m_per_px,
            f64::from(height_px) * m_per_px,
        ]
    }

    /// The size to give the solver for a picture of `width_px` by `height_px`.
    pub fn known_size(&self, width_px: u32, height_px: u32) -> KnownSize {
        let [width_m, height_m] = self.size_m(width_px, height_px);
        KnownSize::Dimensions { width_m, height_m }
    }
}

/// The length of the line the screen draws for the tape: most of the width,
/// long enough that a millimeter on the tape is a small share of it, and
/// always inside what an overscanning TV still shows. Even, so the line can
/// be centered on whole pixels.
pub fn tape_line_px(width_px: u32) -> u32 {
    (width_px * 7 / 10) & !1
}

/// Where the card frame starts, in pixels: from the size the display reports
/// for itself when it does, else from a typical pixel pitch.
pub fn card_start_px(panel_width_mm: Option<f64>, width_px: u32, projected: bool) -> f64 {
    let mm_per_px = match panel_width_mm.filter(|w| w.is_finite() && *w > 0.0) {
        Some(width_mm) if width_px > 0 => width_mm / f64::from(width_px),
        _ if projected => DEFAULT_PROJECTED_MM_PER_PX,
        _ => DEFAULT_FLAT_MM_PER_PX,
    };
    snap(CARD_WIDTH_MM / mm_per_px)
}

/// The widest and narrowest the card frame may get: half to twice the start,
/// and always inside the screen.
pub fn card_bounds_px(start_px: f64, width_px: u32, height_px: u32) -> (f64, f64) {
    let fits_width = f64::from(width_px) * 0.9;
    let fits_height = f64::from(height_px) * 0.8 * CARD_WIDTH_MM / CARD_HEIGHT_MM;
    let hi = (start_px * 2.0).min(fits_width).min(fits_height);
    let lo = (start_px * 0.5).min(hi);
    (snap(lo), snap(hi))
}

/// Rounds to the step the card moves in.
pub fn snap(px: f64) -> f64 {
    (px / CARD_STEP_PX).round() * CARD_STEP_PX
}

/// How far apart two sizes are along the worse axis, as a share of `other`.
pub fn size_gap(reference_m: [f64; 2], other_m: [f64; 2]) -> f64 {
    reference_m
        .iter()
        .zip(other_m)
        .map(|(r, o)| (r / o - 1.0).abs())
        .fold(0.0, f64::max)
}

/// How a hand measurement sits against the size the display reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agreement {
    /// Within what either figure can be trusted to.
    Close,
    /// Enough apart to notice, still within what a display's own figure can be off by.
    Differs,
    /// Far enough that a unit or a reading is the likelier mistake.
    FarOff,
}

/// Classifies a [`size_gap`]. The first limit is the one the camera's own
/// cross-check uses to call two sizes in disagreement.
pub fn agreement(gap: f64) -> Agreement {
    if gap < 0.02 {
        Agreement::Close
    } else if gap < 0.10 {
        Agreement::Differs
    } else {
        Agreement::FarOff
    }
}

/// Reads a length typed from a tape: "47.5", "47,5", "47 5/8", "47-5/8",
/// "121 cm", `47.5"`, "1210mm". A bare number is in `default`.
pub fn parse_length_mm(text: &str, default: Unit) -> Result<f64, ReferenceError> {
    let lower = text.trim().to_lowercase();
    // Longer spellings first, so "inches" is not read as "in" and a bare "m"
    // (meters, which no tape on a screen reads in) is left to fail as a number.
    const UNITS: [(&str, Unit); 8] = [
        ("inches", Unit::Inches),
        ("inch", Unit::Inches),
        ("in", Unit::Inches),
        ("\"", Unit::Inches),
        ("\u{201d}", Unit::Inches),
        ("\u{2033}", Unit::Inches),
        ("mm", Unit::Millimeters),
        ("cm", Unit::Centimeters),
    ];
    let (number, unit) = UNITS
        .iter()
        .find_map(|(suffix, unit)| lower.strip_suffix(suffix).map(|rest| (rest, *unit)))
        .unwrap_or((&lower, default));
    let value = parse_number(number.trim())?;
    Ok(value * unit.mm())
}

fn parse_number(text: &str) -> Result<f64, ReferenceError> {
    let fraction = |part: &str| -> Option<f64> {
        let (top, bottom) = part.split_once('/')?;
        let (top, bottom) = (
            top.trim().parse::<f64>().ok()?,
            bottom.trim().parse::<f64>().ok()?,
        );
        (bottom != 0.0).then_some(top / bottom)
    };
    let value = if text.contains('/') {
        // "5/8", "47 5/8" or "47-5/8".
        match text.split_once(|c: char| c.is_whitespace() || c == '-') {
            Some((whole, part)) => whole
                .trim()
                .parse::<f64>()
                .ok()
                .zip(fraction(part.trim()))
                .map(|(w, f)| w + f),
            None => fraction(text),
        }
    } else {
        text.replace(',', ".").parse::<f64>().ok()
    };
    value
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or(ReferenceError::NotALength)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn a_card_frame_gives_the_pixel_pitch() {
        let r = Reference::from_card(372.5).expect("plausible");
        assert_eq!(r.method, Method::Card);
        assert!(close(r.mm_per_px, 85.6 / 372.5, 1e-12));
    }

    #[test]
    fn a_tape_reading_gives_the_pixel_pitch() {
        let r = Reference::from_tape(1792.0, 413.2).expect("plausible");
        assert!(close(r.mm_per_px, 413.2 / 1792.0, 1e-12));
    }

    #[test]
    fn the_whole_picture_is_the_pitch_times_the_resolution() {
        let r = Reference::from_tape(1792.0, 413.2).unwrap();
        let [w, h] = r.size_m(2560, 1440);
        assert!(close(w, 2560.0 * 413.2 / 1792.0 / 1000.0, 1e-12));
        assert!(close(w / h, 2560.0 / 1440.0, 1e-12));
        assert_eq!(
            r.known_size(2560, 1440),
            KnownSize::Dimensions {
                width_m: w,
                height_m: h
            }
        );
    }

    #[test]
    fn a_reading_that_cannot_be_this_line_is_refused() {
        let line = f64::from(tape_line_px(2560));
        // 470 mm across 1792 px is 0.26 mm per pixel: an ordinary monitor.
        assert!(Reference::from_tape(line, 470.0).is_ok());
        // A centimeter reading typed as millimeters, or a length meant for another line.
        assert_eq!(
            Reference::from_tape(line, 10.0),
            Err(ReferenceError::Implausible)
        );
        assert_eq!(
            Reference::from_tape(line, 1.0e5),
            Err(ReferenceError::Implausible)
        );
        assert_eq!(
            Reference::from_tape(line, 0.0),
            Err(ReferenceError::NotALength)
        );
        assert_eq!(
            Reference::from_card(f64::NAN),
            Err(ReferenceError::NotALength)
        );
        assert_eq!(Reference::from_card(5.0), Err(ReferenceError::Implausible));
    }

    #[test]
    fn lengths_are_read_the_way_they_are_written_on_a_tape() {
        let inch = Unit::Inches;
        let mm = |text| parse_length_mm(text, inch).unwrap();
        assert!(close(mm("47.5"), 47.5 * 25.4, 1e-9));
        assert!(close(mm("47,5"), 47.5 * 25.4, 1e-9));
        assert!(close(mm("47 5/8"), 47.625 * 25.4, 1e-9));
        assert!(close(mm("47-5/8"), 47.625 * 25.4, 1e-9));
        assert!(close(mm("5/8"), 0.625 * 25.4, 1e-9));
        assert!(close(mm("47.5\""), 47.5 * 25.4, 1e-9));
        assert!(close(mm("47.5 inches"), 47.5 * 25.4, 1e-9));
        assert!(close(mm("121 cm"), 1210.0, 1e-9));
        assert!(close(mm("1210mm"), 1210.0, 1e-9));
        // The unit the tech picked applies to a bare number, and a written one beats it.
        assert!(close(
            parse_length_mm("121", Unit::Centimeters).unwrap(),
            1210.0,
            1e-9
        ));
        assert!(close(
            parse_length_mm("121 cm", Unit::Inches).unwrap(),
            1210.0,
            1e-9
        ));
    }

    #[test]
    fn what_is_not_a_length_is_not_one() {
        for bad in [
            "", "  ", "abc", "0", "-3", "1/0", "47 5/", "4 7 5/8", "12 m", "nan", "inf cm",
        ] {
            assert_eq!(
                parse_length_mm(bad, Unit::Inches),
                Err(ReferenceError::NotALength),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_tape_line_is_long_even_and_inside_an_overscanned_picture() {
        for width in [1280, 1920, 2560, 3840, 1366] {
            let line = tape_line_px(width);
            assert_eq!(line % 2, 0, "{width}");
            assert!(
                f64::from(line) >= f64::from(width) * 0.69,
                "{width}: {line}"
            );
            // Even 15% overscan leaves 85% of the picture in view.
            assert!(
                f64::from(line) <= f64::from(width) * 0.85,
                "{width}: {line}"
            );
        }
    }

    #[test]
    fn the_card_starts_at_the_displays_own_size_when_it_has_one() {
        // A 590 mm wide, 2560 px panel: 0.2305 mm per pixel, a card 371.4 px wide.
        let start = card_start_px(Some(590.0), 2560, false);
        assert!(
            close(start, 85.6 / (590.0 / 2560.0), CARD_STEP_PX),
            "{start}"
        );
        // Without one it falls back to a typical pitch, larger for a projected picture.
        assert!(close(
            card_start_px(None, 1920, false),
            85.6 / 0.25,
            CARD_STEP_PX
        ));
        assert!(close(card_start_px(None, 1920, true), 85.6, CARD_STEP_PX));
        assert!(close(
            card_start_px(Some(f64::NAN), 1920, false),
            85.6 / 0.25,
            CARD_STEP_PX
        ));
    }

    #[test]
    fn the_card_can_be_moved_to_half_and_double_but_never_off_the_screen() {
        let (lo, hi) = card_bounds_px(370.0, 2560, 1440);
        assert!(
            close(lo, 185.0, CARD_STEP_PX) && close(hi, 740.0, CARD_STEP_PX),
            "{lo} {hi}"
        );
        // A short screen limits it by height: a card is 1.586 times as wide as tall.
        let (_, hi) = card_bounds_px(600.0, 1280, 400);
        assert!(
            hi <= 400.0 * 0.8 * CARD_WIDTH_MM / CARD_HEIGHT_MM + CARD_STEP_PX,
            "{hi}"
        );
        let (lo, hi) = card_bounds_px(5000.0, 640, 480);
        assert!(lo <= hi && hi <= 640.0 * 0.9, "{lo} {hi}");
    }

    #[test]
    fn a_gap_is_the_worse_axis() {
        assert!(close(size_gap([1.0, 1.0], [1.0, 1.0]), 0.0, 1e-12));
        assert!(close(size_gap([1.006, 1.012], [1.0, 1.0]), 0.012, 1e-9));
        assert!(close(size_gap([0.99, 1.0], [1.0, 1.0]), 0.01, 1e-9));
    }

    #[test]
    fn a_gap_reads_as_close_a_little_off_or_probably_a_mistake() {
        assert_eq!(agreement(0.0), Agreement::Close);
        assert_eq!(agreement(0.0199), Agreement::Close);
        assert_eq!(agreement(0.02), Agreement::Differs);
        assert_eq!(agreement(0.099), Agreement::Differs);
        // Centimeters read as inches is a gap of 154%.
        assert_eq!(agreement(1.54), Agreement::FarOff);
    }
}
