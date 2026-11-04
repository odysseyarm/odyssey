//! Foreign-facing records are validated before conversion: UUIDs have six bytes,
//! firmware versions have three components, accessory keys fit in 48 bits, and
//! present accessory assignments are nonzero. Invalid records produce
//! `ClientError::InvalidArgument`, not a panic or a silently truncated identity.
//!
//! `NotConnected` means no connection has been established. RPC failures retain
//! the original typed gRPC status in `RpcFailure`; local socket and transport
//! failures retain their typed source in `ConnectionFailure`. `StreamEnd` means
//! a closed stream, rather than a server error.
//!
uniffi::setup_scaffolding!();

pub mod client;
pub mod ffi_common;
pub mod funny;
pub mod tracking_history;
