#[derive(Debug)]
pub enum ServerError {
    SocketAlreadyInUse,
    IoError(std::io::Error),
    Other(anyhow::Error),
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

impl From<anyhow::Error> for ServerError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}
