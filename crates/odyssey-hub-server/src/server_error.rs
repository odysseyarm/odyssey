#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("the hub endpoint is already in use")]
    SocketAlreadyInUse,
    #[error("hub I/O failed: {0}")]
    IoError(#[source] std::io::Error),
    #[error("hub transport failed")]
    Transport(#[from] tonic::transport::Error),
    #[error("device tasks exited unexpectedly")]
    DeviceTasksExited,
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
