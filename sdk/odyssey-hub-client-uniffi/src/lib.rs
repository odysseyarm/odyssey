//! Foreign-facing records are validated before conversion. UUIDs have six bytes,
//! firmware versions have three components, accessory keys fit in 48 bits, and
//! present accessory assignments are nonzero. A bad record returns
//! `ClientError::InvalidArgument`, never a panic or a silently truncated identity.
//!
//! `NotConnected` means no connection has been established. `RpcFailure` keeps
//! the original typed gRPC status. `ConnectionFailure` keeps the typed source of
//! local socket and transport failures. `StreamEnd` means the stream closed,
//! which is no server error.
//!
uniffi::setup_scaffolding!();

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
pub mod client;
/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
pub mod ffi_common;
/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
pub mod funny;
/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
pub mod tracking_history;
