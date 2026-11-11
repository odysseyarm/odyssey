//! What the camera thread reports back to the UI.

use std::path::PathBuf;

use odyssey_autocalib::camera::CameraInfo;
use odyssey_autocalib::session::Step;

use super::backend::ListError;
use super::failure::Failure;
use super::frame::{Finished, MarkerFrame, PreviewFrame};

#[derive(Debug)]
pub(crate) enum Event {
    Cameras(Result<Vec<CameraInfo>, ListError>),
    /// Something about the camera session `session`. The UI drops events of
    /// sessions it has already abandoned.
    Session {
        session: u64,
        event: SessionEvent,
    },
}

#[derive(Debug)]
pub(crate) enum SessionEvent {
    Opened,
    Preview(Box<PreviewFrame>),
    Markers(Box<MarkerFrame>),
    Progress(Step),
    ScreenMeasured,
    MarkersMeasured,
    Finished(Box<Finished>),
    Failed(Failure),
    /// The stream is stopped and the camera released.
    Closed,
    /// A diagnostic bundle was written, or could not be.
    AttemptSaved(Result<SavedAttempt, String>),
}

/// A saved diagnostic bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavedAttempt {
    pub path: PathBuf,
    pub bytes: u64,
}
