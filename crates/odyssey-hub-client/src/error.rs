/// Transport error with the gRPC status preserved.
///
/// Renders only the message the server sent; `tonic::Status`'s own Display
/// appends the code, details and gRPC metadata, which ends up verbatim in UI
/// error text. `into_status` hands back the original for anything that needs
/// the code or metadata.
///
/// The status is boxed because it is ~176 bytes, which would otherwise make
/// every `Result<_, RpcError>` and `Result<_, ClientError>` that large.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", rpc_message(.0))]
pub struct RpcError(Box<tonic::Status>);

impl RpcError {
    pub fn into_status(self) -> tonic::Status {
        *self.0
    }
}

impl From<tonic::Status> for RpcError {
    fn from(status: tonic::Status) -> Self {
        Self(Box::new(status))
    }
}

impl From<RpcError> for tonic::Status {
    fn from(error: RpcError) -> Self {
        *error.0
    }
}

fn rpc_message(status: &tonic::Status) -> String {
    let message = status.message().trim();
    if message.is_empty() {
        status.code().to_string()
    } else {
        message.to_owned()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("not connected to the hub")]
    NotConnected,
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Lets RPC methods use `?` straight on a `tonic::Status`.
impl From<tonic::Status> for ClientError {
    fn from(status: tonic::Status) -> Self {
        Self::Rpc(status.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_only_the_trimmed_server_message() {
        let error = RpcError::from(tonic::Status::not_found("  device gone \n"));
        assert_eq!(error.to_string(), "device gone");
    }

    #[test]
    fn displays_the_code_when_the_message_is_blank() {
        let error = RpcError::from(tonic::Status::new(tonic::Code::Unavailable, "  "));
        assert_eq!(error.to_string(), tonic::Code::Unavailable.to_string());
    }

    #[test]
    fn status_survives_the_round_trip() {
        let status = tonic::Status::permission_denied("no");
        let back = RpcError::from(status).into_status();
        assert_eq!(back.code(), tonic::Code::PermissionDenied);
        assert_eq!(back.message(), "no");
    }
}
