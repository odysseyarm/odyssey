//! Plain-language text for everything the calibration core reports as an enum.
//! The core returns data only. This is the one place that decides the words.
//!
//! Every `match` below is exhaustive on purpose: a new `Hint` or `MarkerHint`
//! variant fails to compile here instead of reaching a tech as a blank banner.

use odyssey_autocalib::session::{Circle, Hint, MarkerHint, SideSet, Step};

use super::kind::SizeOrigin;
use super::worker::FailureKind;
use crate::components::wizard::{Icon, Tone};

/// A status banner: a short headline and one actionable sentence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Message {
    pub tone: Tone,
    pub icon: Icon,
    pub title: String,
    pub detail: String,
}

fn message(tone: Tone, icon: Icon, title: &str, detail: &str) -> Message {
    Message {
        tone,
        icon,
        title: title.into(),
        detail: detail.into(),
    }
}

/// What to tell the tech about the camera's view of the dot pattern.
pub(crate) fn screen_hint(hint: Hint) -> Message {
    use Tone::{Attention, Good};
    match hint {
        Hint::Ready => message(
            Good,
            Icon::Check,
            "Looks good",
            "Hold the camera steady and press Next.",
        ),
        Hint::ScreenNotFound => message(
            Attention,
            Icon::Viewfinder,
            "I can't see the screen yet",
            "Point the camera at the screen so the whole dot pattern is in view.",
        ),
        Hint::ScreenPartlyVisible => message(
            Attention,
            Icon::MoveAway,
            "Part of the screen is cut off",
            "Move the camera back a little, or aim it so the whole screen fits in the picture.",
        ),
        Hint::TooDark => message(
            Attention,
            Icon::Moon,
            "The picture is too dark",
            "Move the Brightness slider up, or switch on a light in the room.",
        ),
        Hint::TooBright => message(
            Attention,
            Icon::Sun,
            "The picture is too bright",
            "Move the Brightness slider down, or cut glare from windows and lamps.",
        ),
        Hint::TooClose => message(
            Attention,
            Icon::MoveAway,
            "Move the camera back a little",
            "The camera is too close to the screen to measure it.",
        ),
        Hint::TooFar => message(
            Attention,
            Icon::MoveCloser,
            "Move the camera closer",
            "The screen is too far away for the dots to be seen clearly.",
        ),
        Hint::DepthPoor => message(
            Attention,
            Icon::Cube,
            "The depth sensor can't read the screen well",
            "Keep the camera still and away from direct sunlight and shiny reflections, then try nudging it a little.",
        ),
        Hint::CameraTiltedTooMuch => message(
            Attention,
            Icon::Rotate,
            "The camera is tilted",
            "Level the camera so it isn't pointing up or down or leaning to one side.",
        ),
        Hint::EdgesCropped => message(
            Attention,
            Icon::Crop,
            "The TV is cutting off the edges of the picture",
            "Turn off overscan in the TV's picture settings (look for 'Just Scan', 'Screen Fit' or 'PC mode') so the whole picture shows.",
        ),
        Hint::ScreenTooOblique => message(
            Attention,
            Icon::FaceOn,
            "Face the screen more directly",
            "The camera is looking at the screen from too far to the side. Move it in front of the screen.",
        ),
    }
}

/// What to tell the tech about the tracker markers in the infrared view.
pub(crate) fn marker_hint(hint: MarkerHint) -> Message {
    use Tone::{Attention, Good};
    match hint {
        MarkerHint::Ready => message(
            Good,
            Icon::Check,
            "All 6 markers found",
            "Press Measure when you're ready.",
        ),
        MarkerHint::NoMarkers => message(
            Attention,
            Icon::Bulb,
            "Turn on the tracker markers",
            "No markers are visible. Switch them on and make sure nothing blocks the camera's view of them.",
        ),
        MarkerHint::TooFewMarkers { found } => message(
            Attention,
            Icon::Bulb,
            &format!("Only {found} of 6 markers visible"),
            "Make sure all six markers are on and in the camera's view.",
        ),
        MarkerHint::WrongCount { near_screen } if near_screen > 6 => message(
            Attention,
            Icon::Warning,
            &format!("{near_screen} bright spots on or near the screen, need 6"),
            "Something other than the markers is bright on or next to the screen. Cover it, or lower the marker brightness until only the six markers show.",
        ),
        MarkerHint::WrongCount { near_screen } => message(
            Attention,
            Icon::Warning,
            &format!("Only {near_screen} bright spots on or near the screen, need 6"),
            "Make sure all six markers are on and are on or just around the screen. Markers far from the screen's edge are not counted.",
        ),
        MarkerHint::NoRow => message(
            Attention,
            Icon::Warning,
            "The markers can't be told apart",
            "Three of the six must lie in a nearly straight row, with one clearly between the other two and another marker lower than the row's left end. \"Where the markers can go\" below has the rules.",
        ),
    }
}

/// What to tell the tech when a step fails: a headline and what to do next.
pub(crate) fn failure_message(kind: FailureKind, size_origin: Option<SizeOrigin>) -> Message {
    let (icon, title, detail): (Icon, &str, String) = match kind {
        FailureKind::CameraOpen => (
            Icon::Camera,
            "The camera didn't start",
            "Unplug the camera's USB cable and plug it back in (a USB 3 port directly on the computer works best), then try again.".into(),
        ),
        FailureKind::CameraLost => (
            Icon::Camera,
            "The camera stopped sending pictures",
            "Check the USB cable is pushed in firmly, then try again.".into(),
        ),
        FailureKind::ScreenDots => (
            Icon::Viewfinder,
            "I couldn't see the dot pattern",
            "Keep the camera still with the whole screen in view, then try again.".into(),
        ),
        FailureKind::ScreenDepth => (
            Icon::Cube,
            "The depth sensor couldn't read the screen",
            "Keep the camera steady and away from direct sunlight and reflections on the screen, then try again.".into(),
        ),
        FailureKind::ScreenGeometry => (
            Icon::FaceOn,
            "That doesn't look like a flat, front-facing screen",
            "Make sure the camera faces the front of the screen, the screen is flat, and the dot pattern isn't stretched or covered, then try again.".into(),
        ),
        FailureKind::Markers => (
            Icon::Bulb,
            "The markers weren't where they should be",
            "Turn on all six tracker markers and check they are mounted on this screen, then try again.".into(),
        ),
        FailureKind::EdgesCropped(sides) => (
            Icon::Crop,
            "The TV is cutting off the picture",
            format!(
                "The {} of the picture can't be seen, so the TV's size doesn't match what the camera sees. Turn off overscan in the TV's picture settings ('Just Scan', 'Screen Fit' or 'PC mode'), then try again. If you can't, go back and clear any size you typed so the camera measures it. If the TV reported a size, also tick \"Measure the size with the camera\" there; that works with the picture cut off.",
                sides_words(sides)
            ),
        ),
        FailureKind::SizeInvalid => (
            Icon::Warning,
            "That size doesn't look right",
            "Go back and enter the diagonal in inches, or leave the size empty and let the camera measure it.".into(),
        ),
        FailureKind::SizeShape => match size_origin {
            Some(SizeOrigin::Panel) => (
                Icon::FaceOn,
                "The TV's size doesn't match the shape of the picture",
                "The picture on this display looks stretched or boxed in. Set the display to its normal resolution with no scaling, or go back and enter the size yourself.".into(),
            ),
            _ => (
                Icon::FaceOn,
                "The size you entered doesn't match the shape of the screen",
                "A screen with that size would have a different shape than the picture. Go back and check the size, or leave it empty and let the camera measure it.".into(),
            ),
        },
        FailureKind::Internal => (
            Icon::Warning,
            "Something went wrong",
            "Go back and start again. If it keeps happening, copy the Details and send them to support.".into(),
        ),
    };
    message(Tone::Bad, icon, title, &detail)
}

/// "top and left", "all four edges": which sides of the picture a display cuts off.
pub(crate) fn sides_words(sides: SideSet) -> String {
    if sides.count() == 4 {
        return "edges on all four sides".into();
    }
    let names: Vec<&str> = [
        (sides.top, "top"),
        (sides.right, "right"),
        (sides.bottom, "bottom"),
        (sides.left, "left"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    match names.as_slice() {
        [] => "edges".into(),
        [one] => format!("{one} edge"),
        [first, last] => format!("{first} and {last} edges"),
        [init @ .., last] => format!("{} and {last} edges", init.join(", ")),
    }
}

/// The label of the button on the problem screen that leads on from `kind`.
pub(crate) fn retry_label(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::SizeInvalid | FailureKind::SizeShape => "Change the size",
        _ => "Try again",
    }
}

/// Where to turn off overscan, for the Details section. Menu names change
/// between models and years, so the list says so.
pub(crate) const OVERSCAN_HELP: [(&str, &str); 5] = [
    (
        "Any TV",
        "Look in the picture settings for Picture Size, Aspect Ratio or Screen Fit. Choose 'Just Scan', 'Screen Fit', 'Full Pixel', 'Unscaled' or switch 'Overscan' off. Game mode or PC mode often does this for you.",
    ),
    (
        "Samsung",
        "Picture → Picture Size Settings → Picture Size: Screen Fit, or switch to Game mode.",
    ),
    (
        "LG",
        "Picture → Aspect Ratio → Just Scan (or use the Game picture mode).",
    ),
    ("Sony", "Display Area → Full Pixel."),
    (
        "The computer",
        "In the graphics driver's settings (NVIDIA, AMD or Intel) turn scaling off and set overscan to 0 %.",
    ),
];

/// The circle through markers 1, 2 and 3 in words: where its centre is from the screen's
/// top-left corner, and how big it is. A controller standing straight in front of a point
/// on it is at the weak spot of the tracker's first estimate.
pub(crate) fn circle_text(circle: &Circle) -> String {
    format!(
        "centre {:.0} cm right and {:.0} cm down from the top-left corner, radius {:.0} cm",
        circle.center.x * 100.0,
        circle.center.y * 100.0,
        circle.radius * 100.0
    )
}

/// Where the tracker markers can go and what to avoid, for the markers step. Each item is
/// what the wizard's marker search enforces or what the tracker was run to need. See
/// `crates/odyssey-autocalib/MARKERS.md` and `crates/odyssey-hub-server/tests/marker_order.rs`
/// for where each comes from.
pub(crate) const MARKER_RULES: [(&str, &str); 6] = [
    (
        "Where they can go",
        "On the screen, along its edge, or behind a projector screen. The wizard finds them wherever they are, as long as they are on or just around the screen.",
    ),
    (
        "All six, all at once",
        "All six markers must be on and in the camera's view at the same time. Tracking does not start until it sees all six.",
    ),
    (
        "Nothing else bright",
        "Exactly six bright spots may be on or just around the screen. A lamp, a window or a reflection there counts as a seventh and the wizard can't tell it from a marker: cover it, or lower the marker brightness until only the six markers show. Lights far from the screen are ignored.",
    ),
    (
        "A straight row of three",
        "Three of the six must lie in a nearly straight row, with one clearly between the other two, and no other marker near that line. At least one more marker must be lower than the row's left end. The tracker finds the markers by that row; the numbers on the picture follow from it.",
    ),
    (
        "Not too close together",
        "Markers so close that they look like one spot in the camera are counted as one. Keep the first three markers (the row's two ends and the one below) well spread out: a small or nearly flat triangle makes the start of tracking unreliable.",
    ),
    (
        "Where the controller stands",
        "The tracker starts from markers 1, 2 and 3. A controller whose position, straight in front of the screen, falls on the circle through those three markers (a cylinder through that circle, pointing out from the screen) makes that first estimate ambiguous. It is a weak spot, not a limit: the tracker checks the estimate against all six markers, so it usually still locks on. The review lists the circle's centre and radius.",
    ),
];

/// Text and bar position for a running measurement.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Progress {
    pub text: String,
    /// `0.0..=1.0`.
    pub fraction: f32,
}

/// `what` names what is being measured, e.g. "the screen" or "the markers".
pub(crate) fn progress(step: Step, what: &str) -> Progress {
    match step {
        Step::Capturing { frame, total } => Progress {
            text: format!("Measuring {what}… {} of {}", (frame + 1).min(total), total),
            // Capturing fills most of the bar; solving and done take the rest.
            fraction: 0.85 * frame as f32 / total.max(1) as f32,
        },
        Step::Solving => Progress {
            text: format!("Working out {what}…"),
            fraction: 0.9,
        },
        Step::Done => Progress {
            text: "Done".into(),
            fraction: 1.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_HINTS: [Hint; 11] = [
        Hint::Ready,
        Hint::ScreenNotFound,
        Hint::ScreenPartlyVisible,
        Hint::TooDark,
        Hint::TooBright,
        Hint::TooClose,
        Hint::TooFar,
        Hint::DepthPoor,
        Hint::CameraTiltedTooMuch,
        Hint::ScreenTooOblique,
        Hint::EdgesCropped,
    ];

    #[test]
    fn only_ready_is_good() {
        for hint in ALL_HINTS {
            let m = screen_hint(hint);
            assert_eq!(m.tone == Tone::Good, hint == Hint::Ready, "{hint:?}");
        }
    }

    #[test]
    fn only_ready_markers_are_good() {
        let all = [
            MarkerHint::Ready,
            MarkerHint::NoMarkers,
            MarkerHint::TooFewMarkers { found: 4 },
            MarkerHint::WrongCount { near_screen: 7 },
            MarkerHint::NoRow,
        ];
        for hint in all {
            let m = marker_hint(hint);
            assert_eq!(m.tone == Tone::Good, hint == MarkerHint::Ready, "{hint:?}");
        }
    }

    #[test]
    fn failures_are_bad() {
        let all = [
            FailureKind::CameraOpen,
            FailureKind::CameraLost,
            FailureKind::ScreenDots,
            FailureKind::ScreenDepth,
            FailureKind::ScreenGeometry,
            FailureKind::Markers,
            FailureKind::Internal,
        ];
        let some_sides = SideSet {
            top: true,
            ..SideSet::default()
        };
        let all: Vec<FailureKind> = all
            .into_iter()
            .chain([
                FailureKind::EdgesCropped(some_sides),
                FailureKind::SizeInvalid,
                FailureKind::SizeShape,
            ])
            .collect();
        for kind in all.iter().copied() {
            let m = failure_message(kind, Some(SizeOrigin::Entered));
            assert_eq!(m.tone, Tone::Bad);
        }
    }

    #[test]
    fn progress_is_monotonic_and_ends_full() {
        let steps = [
            Step::Capturing { frame: 0, total: 5 },
            Step::Capturing { frame: 4, total: 5 },
            Step::Solving,
            Step::Done,
        ];
        let fractions: Vec<_> = steps
            .iter()
            .map(|&s| progress(s, "the screen").fraction)
            .collect();
        assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
        assert_eq!(fractions.last(), Some(&1.0));
    }
}
