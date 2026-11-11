//! Protobuf interface of the Odyssey hub server, plus conversions between the
//! generated types and `odyssey-hub-common`.

mod conversions;
mod proto;

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub use proto::*;
