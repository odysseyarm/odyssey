//! What the UI asks the camera thread to do.

use std::path::PathBuf;

use super::backend::OpenRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The stream is open but nothing reads it.
    Hold,
    Preview,
    Markers,
}

#[derive(Debug)]
pub(crate) enum Command {
    ListCameras,
    /// Closes any open session, opens the camera and starts previewing. Events
    /// that follow carry `session`.
    Open {
        session: u64,
        request: OpenRequest,
    },
    SetMode(Mode),
    SetIrExposure(u32),
    MeasureScreen,
    MeasureMarkers,
    /// Computes the result, then closes the camera.
    Finish,
    Close,
    /// Writes the latest attempt's frames, log lines and summary to `path`.
    SaveAttempt {
        path: PathBuf,
    },
}
