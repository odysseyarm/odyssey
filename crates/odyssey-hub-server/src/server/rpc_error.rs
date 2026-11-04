use tonic::Status;

/// Why an RPC could not be served.
///
/// `Status` is large, so helpers return this and the RPC bodies let `?` convert it.
/// The display text is the message the client receives. `task` is "device" or "dongle".
#[derive(Debug, thiserror::Error)]
pub(super) enum RpcError {
    /// A required request field was not set; carries the field's path.
    #[error("{0} missing")]
    MissingField(&'static str),
    /// A wire value cannot be represented by the device protocol.
    #[error("{0} is out of range")]
    InvalidField(&'static str),
    /// No listed device or dongle matches the request.
    #[error("{task}")]
    NotFound { task: &'static str },
    /// The task's command channel is closed or full.
    #[error("{task} task not available")]
    TaskUnavailable { task: &'static str },
    /// The task took the command but dropped its reply channel without answering.
    #[error("{task} task dropped reply")]
    ReplyDropped { task: &'static str },
    /// The task answered with a failure.
    #[error("{0}")]
    TaskFailed(String),
}

impl From<RpcError> for Status {
    fn from(error: RpcError) -> Self {
        let message = error.to_string();
        match error {
            RpcError::MissingField(_) | RpcError::InvalidField(_) => {
                Self::invalid_argument(message)
            }
            RpcError::NotFound { .. } => Self::not_found(message),
            RpcError::TaskUnavailable { .. } => Self::unavailable(message),
            RpcError::ReplyDropped { .. } | RpcError::TaskFailed(_) => Self::internal(message),
        }
    }
}
