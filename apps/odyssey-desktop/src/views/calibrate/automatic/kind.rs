//! What kind of display is being calibrated and what its size is known to be.
//!
//! A flat screen (TV or monitor) reports its physical size in its EDID, which
//! beats the depth camera's measurement as a scale anchor. A projector's EDID
//! says nothing true about the picture, so only the camera can size it. Every
//! decision about that lives here as plain functions: what the lookup found,
//! which kind to preselect, and which size and edge rule the session gets.

use odyssey_autocalib::display::{
    DisplayLookup, EdidInfo, LookupNote, LookupSource, PhysicalSize, Reliability, SizePrecision,
    SuspectReason,
};
use odyssey_autocalib::session::{KnownSize, SizeSource};
pub use odyssey_hub_common::config::DisplayKind;

use super::settings::Entered;

/// Where a size that anchors the measurement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeOrigin {
    /// The display's own EDID.
    Panel,
    /// Typed in by the tech.
    Entered,
}

/// The physical size an EDID reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSize {
    pub width_mm: u32,
    pub height_mm: u32,
    pub precision: SizePrecision,
}

impl PanelSize {
    pub fn diagonal_in(&self) -> f64 {
        f64::from(self.width_mm).hypot(f64::from(self.height_mm)) / 25.4
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FindingKind {
    /// Dimensions that passed every reliability check.
    Found(PanelSize),
    /// Dimensions were reported but should not anchor a measurement.
    Doubtful {
        reason: SuspectReason,
        reported: PanelSize,
    },
    /// The EDID was read and holds no size: only an aspect ratio or nothing.
    /// Projectors report this, and so do some TVs.
    NoSize,
    /// No EDID was found for this monitor.
    Missing(LookupNote),
}

/// What looking up the selected monitor's EDID found.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub kind: FindingKind,
    /// Where the data came from, in words, for the Details section.
    pub source: String,
    /// Whether the size is the demo camera's simulated panel rather than a
    /// real display's.
    pub demo: bool,
}

fn panel_size(info: &EdidInfo) -> Option<PanelSize> {
    match info.physical_size {
        PhysicalSize::Dimensions {
            width_mm,
            height_mm,
            precision,
        } => Some(PanelSize {
            width_mm,
            height_mm,
            precision,
        }),
        PhysicalSize::AspectOnly(_) | PhysicalSize::Undefined => None,
    }
}

fn source_words(source: &LookupSource) -> String {
    match source {
        LookupSource::Sysfs(path) => {
            let connector = path
                .parent()
                .and_then(|p| p.file_name())
                .map_or_else(String::new, |n| format!(" ({})", n.to_string_lossy()));
            format!("The display's own EDID, read from the system's display list{connector}")
        }
        LookupSource::WindowsRegistry(_) => {
            "The display's own EDID, read from the Windows display registry".into()
        }
        LookupSource::Unavailable => "No display information was available".into(),
    }
}

impl Finding {
    pub fn from_lookup(lookup: &DisplayLookup) -> Self {
        let source = source_words(&lookup.source);
        let kind = match &lookup.edid {
            Some(info) => match (panel_size(info), info.reliability) {
                (Some(size), Reliability::Reliable) => FindingKind::Found(size),
                (Some(reported), Reliability::Suspect(reason)) => {
                    FindingKind::Doubtful { reason, reported }
                }
                // Dimensions that are neither reliable nor suspect cannot occur;
                // treat them as no usable size rather than trusting them.
                (Some(_), Reliability::NoDimensions) | (None, _) => FindingKind::NoSize,
            },
            None => FindingKind::Missing(lookup.note.clone().unwrap_or(LookupNote::NotFound)),
        };
        Self {
            kind,
            source,
            demo: false,
        }
    }

    /// The simulated TV the demo camera looks at.
    pub fn demo(width_m: f64, height_m: f64) -> Self {
        let mm = |m: f64| (m * 1000.0).round() as u32;
        Self {
            kind: FindingKind::Found(PanelSize {
                width_mm: mm(width_m),
                height_mm: mm(height_m),
                precision: SizePrecision::Millimeters,
            }),
            source: "Demo mode: the simulated screen's true size".into(),
            demo: true,
        }
    }

    /// The scale anchor the display offers, present only when it can be trusted.
    pub fn known_size(&self) -> Option<KnownSize> {
        match &self.kind {
            FindingKind::Found(p) => Some(KnownSize::Dimensions {
                width_m: f64::from(p.width_mm) / 1000.0,
                height_m: f64::from(p.height_mm) / 1000.0,
            }),
            _ => None,
        }
    }

    /// One sentence on what was found, for the flat-screen question.
    pub fn headline(&self) -> String {
        match &self.kind {
            FindingKind::Found(p) => {
                let size = format!(
                    "{:.0} inches ({:.0} × {:.0} cm)",
                    p.diagonal_in(),
                    f64::from(p.width_mm) / 10.0,
                    f64::from(p.height_mm) / 10.0
                );
                if self.demo {
                    format!("Demo: pretending the TV reports {size}")
                } else {
                    format!("We read the screen size from the TV itself: {size}")
                }
            }
            FindingKind::Doubtful { .. } => {
                "This display reports a size, but it doesn't look right".into()
            }
            FindingKind::NoSize => "This display doesn't report its size".into(),
            FindingKind::Missing(_) => "We couldn't read this display's size".into(),
        }
    }

    /// What to do about it, when the size is not known.
    pub fn advice(&self) -> Option<&'static str> {
        match self.kind {
            FindingKind::Found(_) => None,
            _ => Some(
                "If you can, enter the size printed on the box or the sticker. It's optional but recommended: \
                 without it the camera measures the size itself, which is slightly less accurate.",
            ),
        }
    }

    /// A gentle pointer for displays that report nothing, which is typical of
    /// projectors. Never switches the choice by itself.
    pub fn projector_aside(&self) -> Option<&'static str> {
        matches!(self.kind, FindingKind::NoSize)
            .then_some("Projectors usually don't report a size. If this is a projector, choose Projector above.")
    }

    /// Whether the size is known, which decides whether "Not right?" or the
    /// plain size field is offered.
    pub fn is_found(&self) -> bool {
        matches!(self.kind, FindingKind::Found(_))
    }

    /// Technical lines for the Details section.
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("Source: {}", self.source)];
        match &self.kind {
            FindingKind::Found(p) => {
                lines.push(format!(
                    "Reported size: {} × {} mm",
                    p.width_mm, p.height_mm
                ));
                lines.push(precision_words(p.precision).into());
            }
            FindingKind::Doubtful { reason, reported } => {
                lines.push(format!(
                    "Reported size: {} × {} mm",
                    reported.width_mm, reported.height_mm
                ));
                lines.push(precision_words(reported.precision).into());
                lines.push(format!("Not used because {}", suspect_words(*reason)));
            }
            FindingKind::NoSize => {
                lines.push("The display's information holds no physical size.".into())
            }
            FindingKind::Missing(note) => {
                lines.push(format!("Not used because {}", note_words(note)))
            }
        }
        lines
    }
}

fn precision_words(precision: SizePrecision) -> &'static str {
    match precision {
        SizePrecision::Millimeters => "Precision: millimeters",
        SizePrecision::Centimeters => "Precision: whole centimeters (up to 5 mm off per side)",
    }
}

fn suspect_words(reason: SuspectReason) -> String {
    match reason {
        SuspectReason::TooSmall => "the size is smaller than any real screen.".into(),
        SuspectReason::TooLarge => "the size is larger than any real screen.".into(),
        SuspectReason::AspectPlaceholder => {
            "the numbers look like an aspect ratio (such as 16 by 9), not a size.".into()
        }
        SuspectReason::AspectMismatch {
            size_aspect,
            native_aspect,
        } => {
            format!(
                "the reported shape ({size_aspect:.2}) doesn't match the screen's resolution ({native_aspect:.2})."
            )
        }
    }
}

fn note_words(note: &LookupNote) -> String {
    match note {
        LookupNote::UnsupportedPlatform => "this system can't look up display information.".into(),
        LookupNote::NoMonitorName => "the display has no name to look it up by.".into(),
        LookupNote::NotFound => "no display information matches this monitor.".into(),
        LookupNote::Ambiguous { candidates } => {
            format!("{candidates} displays match this monitor, so none could be picked safely.")
        }
        LookupNote::NoNativeResolution => {
            "the display's information has no resolution to check it against.".into()
        }
        LookupNote::ResolutionMismatch { native, screen } => format!(
            "its resolution ({} × {}) doesn't fit this monitor ({} × {}).",
            native.0, native.1, screen.0, screen.1
        ),
        LookupNote::Unreadable(kind) => {
            format!("the display information couldn't be read ({kind}).")
        }
        LookupNote::Invalid(e) => format!("the display information is invalid ({e})."),
    }
}

/// The kind to preselect: the one used last time on this monitor, else a flat
/// screen. The EDID deliberately does not move the default. A display that
/// reports no size might be a projector or a TV with a stingy EDID, and a
/// wrong silent guess changes how the measurement is anchored.
pub fn default_kind(remembered: Option<DisplayKind>) -> DisplayKind {
    remembered.unwrap_or(DisplayKind::Flat)
}

/// What the session is told about the screen's size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizePlan {
    pub known_size: Option<KnownSize>,
    pub origin: Option<SizeOrigin>,
    /// A size declared for a flat screen describes its panel, so the picture
    /// must be shown whole or the two no longer match. A size the camera
    /// measures needs no such rule: it is the picture's own, hidden edge
    /// included, so a TV that crops its picture can still be calibrated.
    pub require_visible_edges: bool,
}

/// The size and edge rule for a display of `kind`. A typed size wins over the
/// display's own; a projector never uses the display's. `aspect` is the
/// picture's width over height, which turns a typed width into a full size.
/// `prefer_camera` skips the display's own size, which leaves a flat screen
/// exactly as if its EDID had said nothing: the camera measures it, and the
/// picture may be cropped.
pub fn plan(
    kind: DisplayKind,
    finding: Option<&Finding>,
    entered: Option<Entered>,
    aspect: f64,
    prefer_camera: bool,
) -> SizePlan {
    let flat = kind == DisplayKind::Flat;
    if let Some(entered) = entered {
        let known_size = match entered {
            Entered::Diagonal { inches } => KnownSize::Diagonal { inches },
            Entered::Width { cm } => {
                let width_m = cm / 100.0;
                KnownSize::Dimensions {
                    width_m,
                    height_m: width_m / aspect,
                }
            }
        };
        return SizePlan {
            known_size: Some(known_size),
            origin: Some(SizeOrigin::Entered),
            require_visible_edges: flat,
        };
    }
    let panel = match kind {
        DisplayKind::Flat if !prefer_camera => finding.and_then(Finding::known_size),
        DisplayKind::Flat | DisplayKind::Projector => None,
    };
    SizePlan {
        known_size: panel,
        origin: panel.map(|_| SizeOrigin::Panel),
        require_visible_edges: flat && panel.is_some(),
    }
}

/// Where the size on the review screen came from, in words.
pub fn source_label(source: SizeSource, origin: Option<SizeOrigin>) -> &'static str {
    match (source, origin) {
        (SizeSource::Measured, _) => "Measured by the camera",
        (_, Some(SizeOrigin::Entered)) => "From the size you entered",
        (_, Some(SizeOrigin::Panel)) => "From the TV's own information",
        // A declared size always has an origin; the camera's number is the
        // honest fallback if that ever breaks.
        (_, None) => "Measured by the camera",
    }
}

#[cfg(test)]
mod tests {
    use odyssey_autocalib::display::parse_edid;

    use super::*;

    fn lookup(info: Option<EdidInfo>, note: Option<LookupNote>) -> DisplayLookup {
        DisplayLookup {
            edid: info,
            source: LookupSource::Unavailable,
            note,
        }
    }

    fn edid(physical_size: PhysicalSize, reliability: Reliability) -> EdidInfo {
        EdidInfo {
            manufacturer: "TST".into(),
            product_code: 1,
            monitor_name: Some("TEST TV".into()),
            native_resolution: Some((1920, 1080)),
            refresh_hz: Some(60.0),
            physical_size,
            reliability,
            checksums_valid: true,
        }
    }

    fn dims(width_mm: u32, height_mm: u32, precision: SizePrecision) -> PhysicalSize {
        PhysicalSize::Dimensions {
            width_mm,
            height_mm,
            precision,
        }
    }

    #[test]
    fn a_reliable_edid_becomes_a_declared_panel_size() {
        let f = Finding::from_lookup(&lookup(
            Some(edid(
                dims(1210, 680, SizePrecision::Millimeters),
                Reliability::Reliable,
            )),
            None,
        ));
        assert!(f.is_found());
        assert_eq!(
            f.known_size(),
            Some(KnownSize::Dimensions {
                width_m: 1.21,
                height_m: 0.68
            })
        );
        assert_eq!(
            f.headline(),
            "We read the screen size from the TV itself: 55 inches (121 × 68 cm)"
        );
        assert!(f.advice().is_none());
    }

    #[test]
    fn precision_shows_only_in_the_details() {
        let mm = Finding::from_lookup(&lookup(
            Some(edid(
                dims(1210, 680, SizePrecision::Millimeters),
                Reliability::Reliable,
            )),
            None,
        ));
        let cm = Finding::from_lookup(&lookup(
            Some(edid(
                dims(1210, 680, SizePrecision::Centimeters),
                Reliability::Reliable,
            )),
            None,
        ));
        assert!(!mm.headline().to_lowercase().contains("precision"));
        assert!(mm.detail_lines().iter().any(|l| l.contains("millimeters")));
        assert!(cm.detail_lines().iter().any(|l| l.contains("5 mm")));
    }

    #[test]
    fn a_suspect_edid_is_not_an_anchor_and_says_why() {
        let reasons = [
            SuspectReason::AspectPlaceholder,
            SuspectReason::TooSmall,
            SuspectReason::TooLarge,
            SuspectReason::AspectMismatch {
                size_aspect: 1.6,
                native_aspect: 1.78,
            },
        ];
        for reason in reasons {
            let f = Finding::from_lookup(&lookup(
                Some(edid(
                    dims(160, 90, SizePrecision::Centimeters),
                    Reliability::Suspect(reason),
                )),
                None,
            ));
            assert!(matches!(f.kind, FindingKind::Doubtful { .. }), "{reason:?}");
            assert_eq!(f.known_size(), None);
            assert!(f.advice().is_some());
            assert!(
                f.detail_lines()
                    .iter()
                    .any(|l| l.starts_with("Not used because")),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn displays_without_a_size_are_not_guessed_to_be_projectors() {
        for physical in [PhysicalSize::Undefined, PhysicalSize::AspectOnly(1.78)] {
            let f = Finding::from_lookup(&lookup(
                Some(edid(physical, Reliability::NoDimensions)),
                None,
            ));
            assert_eq!(f.kind, FindingKind::NoSize);
            assert_eq!(f.known_size(), None);
            assert!(f.projector_aside().is_some());
            assert_eq!(default_kind(None), DisplayKind::Flat);
        }
    }

    #[test]
    fn a_missing_edid_names_the_lookup_note() {
        for note in [
            LookupNote::NotFound,
            LookupNote::NoMonitorName,
            LookupNote::UnsupportedPlatform,
            LookupNote::Ambiguous { candidates: 2 },
            LookupNote::NoNativeResolution,
            LookupNote::ResolutionMismatch {
                native: (3840, 2160),
                screen: (1280, 1024),
            },
            LookupNote::Unreadable(std::io::ErrorKind::PermissionDenied),
        ] {
            let f = Finding::from_lookup(&lookup(None, Some(note.clone())));
            assert_eq!(f.kind, FindingKind::Missing(note));
            assert!(f.advice().is_some() && f.projector_aside().is_none());
        }
        assert!(matches!(
            Finding::from_lookup(&lookup(None, None)).kind,
            FindingKind::Missing(LookupNote::NotFound)
        ));
    }

    #[test]
    fn a_parsed_projector_style_edid_reads_as_no_size() {
        // Both size bytes zero, as a projector reports.
        let mut bytes = vec![0u8; 128];
        bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        bytes[18] = 1;
        bytes[19] = 4;
        let sum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        bytes[127] = 0u8.wrapping_sub(sum);
        let info = parse_edid(&bytes).unwrap();
        assert_eq!(
            Finding::from_lookup(&lookup(Some(info), None)).kind,
            FindingKind::NoSize
        );
    }

    #[test]
    fn the_remembered_kind_wins_and_flat_is_the_default() {
        assert_eq!(default_kind(None), DisplayKind::Flat);
        assert_eq!(
            default_kind(Some(DisplayKind::Projector)),
            DisplayKind::Projector
        );
        assert_eq!(default_kind(Some(DisplayKind::Flat)), DisplayKind::Flat);
    }

    fn found() -> Finding {
        Finding::from_lookup(&lookup(
            Some(edid(
                dims(1210, 680, SizePrecision::Millimeters),
                Reliability::Reliable,
            )),
            None,
        ))
    }

    #[test]
    fn flat_screens_anchor_to_the_panel_and_require_their_edges() {
        let p = plan(DisplayKind::Flat, Some(&found()), None, 16.0 / 9.0, false);
        assert_eq!(
            p.known_size,
            Some(KnownSize::Dimensions {
                width_m: 1.21,
                height_m: 0.68
            })
        );
        assert_eq!(p.origin, Some(SizeOrigin::Panel));
        assert!(p.require_visible_edges);
    }

    #[test]
    fn a_typed_size_beats_the_panel() {
        let p = plan(
            DisplayKind::Flat,
            Some(&found()),
            Some(Entered::Diagonal { inches: 40.0 }),
            16.0 / 9.0,
            false,
        );
        assert_eq!(p.known_size, Some(KnownSize::Diagonal { inches: 40.0 }));
        assert_eq!(p.origin, Some(SizeOrigin::Entered));
        assert!(p.require_visible_edges);
    }

    #[test]
    fn preferring_the_camera_plans_a_flat_screen_like_one_whose_edid_said_nothing() {
        let p = plan(DisplayKind::Flat, Some(&found()), None, 16.0 / 9.0, true);
        assert_eq!((p.known_size, p.origin), (None, None));
        assert!(!p.require_visible_edges);
    }

    #[test]
    fn a_typed_size_still_wins_when_the_camera_is_preferred() {
        let p = plan(
            DisplayKind::Flat,
            Some(&found()),
            Some(Entered::Diagonal { inches: 40.0 }),
            16.0 / 9.0,
            true,
        );
        assert_eq!(p.known_size, Some(KnownSize::Diagonal { inches: 40.0 }));
        assert_eq!(p.origin, Some(SizeOrigin::Entered));
        assert!(p.require_visible_edges);
    }

    #[test]
    fn a_flat_screen_without_a_size_is_measured_and_may_crop_its_picture() {
        let none = Finding::from_lookup(&lookup(None, Some(LookupNote::NotFound)));
        let p = plan(DisplayKind::Flat, Some(&none), None, 16.0 / 9.0, false);
        assert_eq!((p.known_size, p.origin), (None, None));
        assert!(!p.require_visible_edges);
        assert_eq!(
            plan(DisplayKind::Flat, None, None, 16.0 / 9.0, false).known_size,
            None
        );
    }

    #[test]
    fn projectors_ignore_the_display_and_never_require_edges() {
        let p = plan(
            DisplayKind::Projector,
            Some(&found()),
            None,
            16.0 / 9.0,
            false,
        );
        assert_eq!(
            (p.known_size, p.origin, p.require_visible_edges),
            (None, None, false)
        );
        let typed = plan(
            DisplayKind::Projector,
            Some(&found()),
            Some(Entered::Diagonal { inches: 100.0 }),
            16.0 / 9.0,
            false,
        );
        assert_eq!(
            typed.known_size,
            Some(KnownSize::Diagonal { inches: 100.0 })
        );
        assert!(!typed.require_visible_edges);
    }

    #[test]
    fn a_typed_width_takes_its_height_from_the_pictures_shape() {
        let p = plan(
            DisplayKind::Projector,
            None,
            Some(Entered::Width { cm: 240.0 }),
            16.0 / 9.0,
            false,
        );
        let Some(KnownSize::Dimensions { width_m, height_m }) = p.known_size else {
            panic!("{p:?}")
        };
        assert!(
            (width_m - 2.4).abs() < 1e-9 && (height_m - 1.35).abs() < 1e-9,
            "{width_m} x {height_m}"
        );
    }

    #[test]
    fn the_size_source_is_told_in_plain_words() {
        assert_eq!(
            source_label(SizeSource::KnownDimensions, Some(SizeOrigin::Panel)),
            "From the TV's own information"
        );
        assert_eq!(
            source_label(SizeSource::KnownDiagonal, Some(SizeOrigin::Entered)),
            "From the size you entered"
        );
        assert_eq!(
            source_label(SizeSource::KnownDimensions, Some(SizeOrigin::Entered)),
            "From the size you entered"
        );
        assert_eq!(
            source_label(SizeSource::Measured, None),
            "Measured by the camera"
        );
    }

    #[test]
    fn the_demo_finding_is_marked_as_a_demo() {
        let f = Finding::demo(1.2, 0.675);
        assert!(f.demo && f.is_found());
        assert!(f.headline().starts_with("Demo:"));
    }
}
