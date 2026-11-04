//! Conversions between generated protobuf types and `odyssey-hub-common`.
//!
//! Inbound messages with required fields or enums use `TryFrom` so malformed or
//! newer wire data is reported to the caller rather than panicking.

mod accessory;
mod device;
mod event;
mod hardware_address;
mod image;
mod math;
mod screen;
