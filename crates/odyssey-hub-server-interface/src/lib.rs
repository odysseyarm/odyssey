//! Protobuf interface of the Odyssey hub server, plus conversions between the
//! generated types and `odyssey-hub-common`.

mod conversions;
mod proto;

pub use proto::*;
