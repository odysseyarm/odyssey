//! Which kind to preselect, and which size and edge rule the session gets.

use odyssey_autocalib::session::{KnownSize, SizeSource};

use super::DisplayKind;
use super::finding::Finding;
use crate::views::calibrate::automatic::settings::Entered;

/// Where a size that anchors the measurement came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SizeOrigin {
    /// The display's own EDID.
    Panel,
    /// Typed in by the tech.
    Entered,
    /// Measured by hand with a tape or a credit card: the size of the picture
    /// as displayed, hidden edge included.
    Reference,
}

/// The kind to preselect: the one used last time on this monitor, else a flat
/// screen. The EDID deliberately does not move the default. A display that
/// reports no size might be a projector or a TV with a stingy EDID, and a
/// wrong silent guess changes how the measurement is anchored.
pub(crate) fn default_kind(remembered: Option<DisplayKind>) -> DisplayKind {
    remembered.unwrap_or(DisplayKind::Flat)
}

/// What the session is told about the screen's size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SizePlan {
    pub known_size: Option<KnownSize>,
    pub origin: Option<SizeOrigin>,
    /// A size declared for a flat screen describes its panel, so the picture
    /// must be shown whole or the two do not match. A size the camera
    /// measures needs no such rule: it is the picture's own, hidden edge
    /// included, so a TV that crops its picture can still be calibrated.
    pub require_visible_edges: bool,
}

/// The size and edge rule for a display of `kind`. A typed size wins, then a
/// size measured by hand (`reference`), then the display's own. A projector
/// never uses the display's. `aspect` is the picture's width over height,
/// which turns a typed width into a full size. `prefer_camera` skips the
/// display's own size, which leaves a flat screen exactly as if its EDID had
/// said nothing: the camera measures it, and the picture may be cropped.
pub(crate) fn plan(
    kind: DisplayKind,
    finding: Option<&Finding>,
    entered: Option<Entered>,
    aspect: f64,
    prefer_camera: bool,
    reference: Option<KnownSize>,
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
    if let Some(reference) = reference {
        // A hand measurement is of the picture itself, so a cropped edge does
        // not make it wrong.
        return SizePlan {
            known_size: Some(reference),
            origin: Some(SizeOrigin::Reference),
            require_visible_edges: false,
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
pub(crate) fn source_label(source: SizeSource, origin: Option<SizeOrigin>) -> &'static str {
    match (source, origin) {
        (SizeSource::Measured, _) | (_, None) => "Measured by the camera",
        (_, Some(SizeOrigin::Entered)) => "From the size you entered",
        (_, Some(SizeOrigin::Reference)) => "From your tape or card measurement",
        (_, Some(SizeOrigin::Panel)) => "From the TV's own information",
        // A declared size always has an origin. The camera's number is the
        // honest fallback if that ever breaks.
    }
}
