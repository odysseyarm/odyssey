/// Transport error with the gRPC status preserved.
///
/// Renders only the message the server sent; `tonic::Status`'s own Display
/// appends the code, details and gRPC metadata, which ends up verbatim in UI
/// error text. `into_status` hands back the original for anything that needs
/// the code or metadata.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", rpc_message(.0))]
pub struct RpcError(tonic::Status);

impl RpcError {
    pub fn into_status(self) -> tonic::Status {
        self.0
    }
}

impl From<tonic::Status> for RpcError {
    fn from(status: tonic::Status) -> Self {
        Self(status)
    }
}

impl From<RpcError> for tonic::Status {
    fn from(error: RpcError) -> Self {
        error.0
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
