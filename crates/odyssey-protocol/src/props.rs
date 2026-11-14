use odyssey_protocol_macros::protocol_type;

use crate::{DeviceName, Version};

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropKind {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(0))]
    Uuid = 0,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(1))]
    ProductId = 1,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(2))]
    Name = 2,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(3))]
    Version = 3,
    /// The release counter of the net core's firmware, `0` when the net core
    /// predates reporting or has not answered. Gate reads on a device protocol
    /// of 0.1.5 or newer: older firmware cannot decode the request.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    ///
    /// Wire baseline: device protocol 0.1.5.
    #[cfg_attr(feature = "minicbor", n(4))]
    NetCoreRelease = 4,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Debug, PartialEq)]
pub enum Props {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(0))]
    Uuid(#[cfg_attr(feature = "minicbor", n(0))] [u8; 6]),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(1))]
    ProductId(#[cfg_attr(feature = "minicbor", n(0))] u16),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(2))]
    Name(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::heapless_str32"))]
        DeviceName,
    ),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(3))]
    Version(#[cfg_attr(feature = "minicbor", n(0))] Version),
    /// Since: Odyssey Desktop 0.6.4.
    ///
    /// Wire baseline: device protocol 0.1.5.
    #[cfg_attr(feature = "minicbor", n(4))]
    NetCoreRelease(#[cfg_attr(feature = "minicbor", n(0))] u16),
}
