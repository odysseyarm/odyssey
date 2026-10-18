//! Plain-language text for everything the calibration core reports as an enum.
//! The core returns data only; this is the one place that decides the words.
//!
//! Every `match` below is exhaustive on purpose: a new `Hint` or `MarkerHint`
//! variant fails to compile here instead of reaching a tech as a blank banner.

use odyssey_autocalib::session::{Hint, MarkerHint, Step};

use super::worker::FailureKind;
use crate::components::wizard::{Icon, Tone};

/// A status banner: a short headline and one actionable sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub tone: Tone,
    pub icon: Icon,
    pub title: String,
    pub detail: String,
}

fn message(tone: Tone, icon: Icon, title: &str, detail: &str) -> Message {
    Message { tone, icon, title: title.into(), detail: detail.into() }
}

/// What to tell the tech about the camera's view of the dot pattern.
pub fn screen_hint(hint: Hint) -> Message {
    use Tone::{Attention, Good};
    match hint {
        Hint::Ready => message(Good, Icon::Check, "Looks good", "Hold the camera steady and press Next."),
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
        Hint::ScreenTooOblique => message(
            Attention,
            Icon::FaceOn,
            "Face the screen more directly",
            "The camera is looking at the screen from too far to the side. Move it in front of the screen.",
        ),
    }
}

/// What to tell the tech about the tracker markers in the infrared view.
pub fn marker_hint(hint: MarkerHint) -> Message {
    use Tone::{Attention, Good};
    match hint {
        MarkerHint::Ready => message(Good, Icon::Check, "All 6 markers found", "Hold still. Measuring starts by itself."),
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
        MarkerHint::WrongPlace { matched } => message(
            Attention,
            Icon::Warning,
            &format!("Only {matched} of 6 markers are where they should be"),
            "Check the markers are mounted on this screen, and that no other bright lights are in the camera's view.",
        ),
    }
}

/// What to tell the tech when a step fails: a headline and what to do next.
pub fn failure_message(kind: FailureKind) -> Message {
    let (icon, title, detail) = match kind {
        FailureKind::CameraOpen => (
            Icon::Camera,
            "The camera didn't start",
            "Unplug the camera's USB cable and plug it back in (a USB 3 port directly on the computer works best), then try again.",
        ),
        FailureKind::CameraLost => (
            Icon::Camera,
            "The camera stopped sending pictures",
            "Check the USB cable is pushed in firmly, then try again.",
        ),
        FailureKind::ScreenDots => (
            Icon::Viewfinder,
            "I couldn't see the dot pattern",
            "Keep the camera still with the whole screen in view, then try again.",
        ),
        FailureKind::ScreenDepth => (
            Icon::Cube,
            "The depth sensor couldn't read the screen",
            "Keep the camera steady and away from direct sunlight and reflections on the screen, then try again.",
        ),
        FailureKind::ScreenGeometry => (
            Icon::FaceOn,
            "That doesn't look like a flat, front-facing screen",
            "Make sure the camera faces the front of the screen, the screen is flat, and the dot pattern isn't stretched or covered, then try again.",
        ),
        FailureKind::Markers => (
            Icon::Bulb,
            "The markers weren't where they should be",
            "Turn on all six tracker markers and check they are mounted on this screen, then try again.",
        ),
        FailureKind::Internal => (
            Icon::Warning,
            "Something went wrong",
            "Go back and start again. If it keeps happening, copy the Details and send them to support.",
        ),
    };
    message(Tone::Bad, icon, title, detail)
}

/// Text and bar position for a running measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub text: String,
    /// `0.0..=1.0`.
    pub fraction: f32,
}

/// `what` names what is being measured, e.g. "the screen" or "the markers".
pub fn progress(step: Step, what: &str) -> Progress {
    match step {
        Step::Capturing { frame, total } => Progress {
            text: format!("Measuring {what}… {} of {}", (frame + 1).min(total), total),
            // Capturing fills most of the bar; solving and done take the rest.
            fraction: 0.85 * frame as f32 / total.max(1) as f32,
        },
        Step::Solving => Progress { text: format!("Working out {what}…"), fraction: 0.9 },
        Step::Done => Progress { text: "Done".into(), fraction: 1.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_HINTS: [Hint; 10] = [
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
    ];

    #[test]
    fn every_hint_has_words_and_only_ready_is_good() {
        for hint in ALL_HINTS {
            let m = screen_hint(hint);
            assert!(!m.title.trim().is_empty() && !m.detail.trim().is_empty(), "{hint:?}");
            assert_eq!(m.tone == Tone::Good, hint == Hint::Ready, "{hint:?}");
        }
    }

    #[test]
    fn hints_are_told_apart() {
        let mut titles: Vec<_> = ALL_HINTS.iter().map(|&h| screen_hint(h).title).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), ALL_HINTS.len());
    }

    #[test]
    fn distance_advice_points_the_right_way() {
        assert!(screen_hint(Hint::TooClose).title.contains("back"));
        assert!(screen_hint(Hint::TooFar).title.contains("closer"));
    }

    #[test]
    fn every_marker_hint_has_words_and_counts_show() {
        let all = [
            MarkerHint::Ready,
            MarkerHint::NoMarkers,
            MarkerHint::TooFewMarkers { found: 4 },
            MarkerHint::WrongPlace { matched: 2 },
        ];
        for hint in all {
            let m = marker_hint(hint);
            assert!(!m.title.trim().is_empty() && !m.detail.trim().is_empty(), "{hint:?}");
        }
        assert!(marker_hint(MarkerHint::TooFewMarkers { found: 4 }).title.contains('4'));
        assert!(marker_hint(MarkerHint::WrongPlace { matched: 2 }).title.contains('2'));
        assert_eq!(marker_hint(MarkerHint::Ready).tone, Tone::Good);
    }

    #[test]
    fn every_failure_has_advice() {
        let all = [
            FailureKind::CameraOpen,
            FailureKind::CameraLost,
            FailureKind::ScreenDots,
            FailureKind::ScreenDepth,
            FailureKind::ScreenGeometry,
            FailureKind::Markers,
            FailureKind::Internal,
        ];
        let mut titles = Vec::new();
        for kind in all {
            let m = failure_message(kind);
            assert!(!m.title.trim().is_empty() && m.detail.ends_with("."), "{kind:?}");
            assert_eq!(m.tone, Tone::Bad);
            titles.push(m.title);
        }
        titles.sort();
        titles.dedup();
        assert_eq!(titles.len(), all.len());
    }

    #[test]
    fn progress_is_monotonic_and_ends_full() {
        let steps = [
            Step::Capturing { frame: 0, total: 5 },
            Step::Capturing { frame: 4, total: 5 },
            Step::Solving,
            Step::Done,
        ];
        let fractions: Vec<_> = steps.iter().map(|&s| progress(s, "the screen").fraction).collect();
        assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
        assert_eq!(fractions.last(), Some(&1.0));
        assert!(progress(steps[0], "the screen").text.contains("1 of 5"));
        assert!(progress(steps[1], "the screen").text.contains("5 of 5"));
    }
}
