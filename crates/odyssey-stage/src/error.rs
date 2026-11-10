use odyssey_sim::dataset::DatasetError;
use odyssey_sim::surface::SurfaceError;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StageError {
    #[error("{0}")]
    Invalid(String),
    #[error("display geometry: {0}")]
    Surface(#[from] SurfaceError),
    #[error("scene: {0}")]
    Scene(#[from] DatasetError),
    #[error("no device with id {0:?}")]
    UnknownDevice(String),
    #[error("device {0:?} is not a tracker")]
    NotATracker(String),
}

impl StageError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}
