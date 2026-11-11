/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("the hub endpoint is already in use")]
    SocketAlreadyInUse,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("hub I/O failed: {0}")]
    IoError(#[source] std::io::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("hub transport failed")]
    Transport(#[from] tonic::transport::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("device tasks exited unexpectedly")]
    DeviceTasksExited,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg(feature = "sim")]
    #[error("simulator control failed: {0}")]
    Simulator(#[from] crate::sim::ControlError),
}

impl From<std::io::Error> for ServerError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::AddrInUse {
            Self::SocketAlreadyInUse
        } else {
            Self::IoError(err)
        }
    }
}
