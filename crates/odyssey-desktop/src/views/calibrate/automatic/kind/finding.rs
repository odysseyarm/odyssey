//! What the display's own information (its EDID) says about its size, in the
//! words Setup shows.

use odyssey_autocalib::display::{
    DisplayLookup, EdidInfo, LookupNote, LookupSource, PhysicalSize, Reliability, SizePrecision,
    SuspectReason,
};
use odyssey_autocalib::session::KnownSize;

/// The physical size an EDID reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PanelSize {
    pub width_mm: u32,
    pub height_mm: u32,
    pub precision: SizePrecision,
}

impl PanelSize {
    pub(crate) fn diagonal_in(&self) -> f64 {
        f64::from(self.width_mm).hypot(f64::from(self.height_mm)) / 25.4
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FindingKind {
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
pub(crate) struct Finding {
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
    pub(crate) fn from_lookup(lookup: &DisplayLookup) -> Self {
        let source = source_words(&lookup.source);
        let kind = match &lookup.edid {
            Some(info) => match (panel_size(info), info.reliability) {
                (Some(size), Reliability::Reliable) => FindingKind::Found(size),
                (Some(reported), Reliability::Suspect(reason)) => {
                    FindingKind::Doubtful { reason, reported }
                }
                // Dimensions that are neither reliable nor suspect cannot occur.
                // Treat them as no usable size instead of trusting them.
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
    pub(crate) fn demo(width_m: f64, height_m: f64) -> Self {
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
    pub(crate) fn known_size(&self) -> Option<KnownSize> {
        match &self.kind {
            FindingKind::Found(p) => Some(KnownSize::Dimensions {
                width_m: f64::from(p.width_mm) / 1000.0,
                height_m: f64::from(p.height_mm) / 1000.0,
            }),
            _ => None,
        }
    }

    /// One sentence on what was found, for the flat-screen question.
    pub(crate) fn headline(&self) -> String {
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
    pub(crate) fn advice(&self) -> Option<&'static str> {
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
    pub(crate) fn projector_aside(&self) -> Option<&'static str> {
        matches!(self.kind, FindingKind::NoSize)
            .then_some("Projectors usually don't report a size. If this is a projector, choose Projector above.")
    }

    /// Whether the size is known, which decides whether "Not right?" or the
    /// plain size field is offered.
    pub(crate) fn is_found(&self) -> bool {
        matches!(self.kind, FindingKind::Found(_))
    }

    /// Technical lines for the Details section.
    pub(crate) fn detail_lines(&self) -> Vec<String> {
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
                lines.push("The display's information holds no physical size.".into());
            }
            FindingKind::Missing(note) => {
                lines.push(format!("Not used because {}", note_words(note)));
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
