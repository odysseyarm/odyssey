use odyssey_sim::dataset::DatasetError;
use odyssey_sim::surface::SurfaceError;

/// Since: Odyssey Desktop 0.7.5.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    /// Since: Odyssey Desktop 0.7.5.
    #[error("{0}")]
    Invalid(String),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("display geometry: {0}")]
    Surface(#[from] SurfaceError),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("scene: {0}")]
    Scene(#[from] DatasetError),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("no device with id {0:?}")]
    UnknownDevice(String),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("device {0:?} is not a tracker")]
    NotATracker(String),
}

impl StageError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}
