//! Why a session ended, in categories coarse enough to give advice on.

use odyssey_autocalib::session::{SessionError, SideSet};
use odyssey_autocalib::solve::SolveError;

use crate::views::calibrate::automatic::kind::SizeOrigin;

/// What went wrong, coarsely enough to give advice; `details` keeps the
/// technical text for the Details section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub kind: FailureKind,
    pub details: String,
    /// Whether the camera is still open and a retry can reuse it.
    pub camera_open: bool,
    /// Where the declared size came from, which changes what to tell the tech
    /// when it is refused.
    pub size_origin: Option<SizeOrigin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// The camera would not start.
    CameraOpen,
    /// The camera stopped delivering frames or reported an error.
    CameraLost,
    /// The pattern was not found while measuring the screen.
    ScreenDots,
    /// The depth data did not describe a flat screen.
    ScreenDepth,
    /// The measured screen cannot be right: mirrored, edge-on, or dots that do
    /// not agree with a flat panel.
    ScreenGeometry,
    /// The markers could not be found at their places.
    Markers,
    /// The display cuts off the picture's border on these sides.
    EdgesCropped(SideSet),
    /// The declared size is not a usable size.
    SizeInvalid,
    /// The declared size has a different shape than the picture.
    SizeShape,
    Internal,
}

impl FailureKind {
    pub(super) fn of_solve(error: &SolveError) -> Self {
        match error {
            SolveError::PatternNotFound(_) => Self::ScreenDots,
            SolveError::TooFewDepthPoints { .. } | SolveError::NoPlane => Self::ScreenDepth,
            SolveError::DegeneratePose | SolveError::Mirrored | SolveError::DotsInconsistent => {
                Self::ScreenGeometry
            }
            SolveError::KnownSizeInvalid => Self::SizeInvalid,
            SolveError::KnownSizeAspectMismatch { .. } => Self::SizeShape,
        }
    }

    pub(super) fn of_session(error: &SessionError) -> Self {
        match error {
            SessionError::Camera(_) => Self::CameraLost,
            SessionError::Solve(e) => Self::of_solve(e),
            SessionError::Markers(_) => Self::Markers,
            SessionError::EdgesCropped(sides) => Self::EdgesCropped(*sides),
            SessionError::ScreenNotMeasured | SessionError::MarkersNotMeasured => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_autocalib::camera::CameraError;

    use super::*;

    #[test]
    fn solve_errors_map_to_advice_categories() {
        use odyssey_autocalib::pattern::DetectFailure;
        let kind = |e: SolveError| FailureKind::of_session(&SessionError::Solve(e));
        assert_eq!(
            kind(SolveError::PatternNotFound(DetectFailure::NoDots {
                blobs: 0
            })),
            FailureKind::ScreenDots
        );
        assert_eq!(kind(SolveError::NoPlane), FailureKind::ScreenDepth);
        assert_eq!(
            kind(SolveError::TooFewDepthPoints { found: 3 }),
            FailureKind::ScreenDepth
        );
        assert_eq!(kind(SolveError::Mirrored), FailureKind::ScreenGeometry);
        assert_eq!(
            FailureKind::of_session(&SessionError::Camera(CameraError::Timeout)),
            FailureKind::CameraLost
        );
        assert_eq!(kind(SolveError::KnownSizeInvalid), FailureKind::SizeInvalid);
        assert_eq!(
            kind(SolveError::KnownSizeAspectMismatch {
                known_aspect: 1.6,
                screen_aspect: 1.78
            }),
            FailureKind::SizeShape
        );
        let sides = SideSet {
            top: true,
            left: true,
            ..SideSet::default()
        };
        assert_eq!(
            FailureKind::of_session(&SessionError::EdgesCropped(sides)),
            FailureKind::EdgesCropped(sides)
        );
    }
}
