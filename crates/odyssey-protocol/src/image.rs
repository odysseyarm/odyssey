//! USB-only grayscale capture. Modern firmware resumes tracking without a reboot, legacy needs one.
//!
//! Start reserves one request id: its status, optional objects and pixel chunks all
//! use that id. Object samples, when present, precede pixels and are indexed from
//! zero. Image-only capture sends zero object count and timestamp. Every pixel chunk
//! repeats the complete frame header. Offsets are contiguous byte offsets in a
//! row-major, native-camera-coordinate grayscale image. Stop uses a separate
//! request id. A stale Stop acknowledges its generation without stopping a newer
//! capture. Generations isolate cancelled sessions, including reused request ids.

use odyssey_protocol_macros::protocol_type;

use crate::Port;

/// Since: Odyssey Desktop 0.7.0.
pub const IMAGE_CHUNK_CAPACITY: usize = 192;
/// Since: Odyssey Desktop 0.7.0.
pub const MAX_IMAGE_OBJECTS: usize = 16;
/// Since: Odyssey Desktop 0.7.0.
pub const MAX_IMAGE_PIXELS: usize = 320 * 240;
/// Modern firmware ends capture and restores tracking when no same-generation
/// Renew arrives within this window. Start grants the first lease.
///
/// Since: Odyssey Desktop 0.7.0.
pub const IMAGE_LEASE_TIMEOUT_MS: u64 = 5000;
/// Viewer renewal cadence, which leaves several missed renewals before expiry.
///
/// Since: Odyssey Desktop 0.7.0.
pub const IMAGE_LEASE_RENEW_INTERVAL_MS: u64 = 1000;

/// One camera at a time, requesting 1..=15 FPS. Acquisition and USB backpressure
/// can lower the delivered rate. Not supported over BLE.
///
/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageStreamConfig {
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub camera: Port,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub fps: u8,
}

/// Diagnostics reported by the sensor, separate from the tracking acceptance criteria.
///
/// Coordinates are native image pixels. The normalized tracking grid does not apply.
/// `bounds`, when available, is [left, top, right, bottom]. A missing sensor field
/// is None, never an invented value.
///
/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectFeature {
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub id: u8,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub x: f32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub y: f32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub area: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(4))]
    pub mean_brightness: u8,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(5))]
    pub max_brightness: Option<u8>,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(6))]
    pub bounds: Option<[u16; 4]>,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(7))]
    pub radius: Option<f32>,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(8))]
    pub aspect_ratio: Option<f32>,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(9))]
    pub is_circle: Option<bool>,
}

/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageCommand {
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(0))]
    Start {
        /// Since: Odyssey Desktop 0.7.0.
        #[cfg_attr(feature = "minicbor", n(0))]
        generation: u32,
        /// Since: Odyssey Desktop 0.7.0.
        #[cfg_attr(feature = "minicbor", n(1))]
        config: ImageStreamConfig,
    },
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(1))]
    Stop {
        /// Since: Odyssey Desktop 0.7.0.
        #[cfg_attr(feature = "minicbor", n(0))]
        generation: u32,
    },
    /// Extends the active session's lease. Never answered, and stale generations are ignored.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(2))]
    Renew {
        /// Since: Odyssey Desktop 0.7.0.
        #[cfg_attr(feature = "minicbor", n(0))]
        generation: u32,
    },
}

/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(eq, eq_int))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageStatusCode {
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(0))]
    Started,
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(1))]
    Stopped,
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(2))]
    UnsupportedCamera,
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(3))]
    UsbRequired,
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(4))]
    Busy,
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(5))]
    CaptureError,
    /// Firmware does not implement image capture.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(6))]
    Unsupported,
    /// Pixel transfer ended, but legacy sensor image mode requires reboot/reconnect.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(7))]
    RebootRequired,
    /// No Renew arrived in time, so modern firmware restored tracking.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(8))]
    LeaseExpired,
}

/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageStatus {
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub generation: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub status: ImageStatusCode,
}

/// A bounded pixel payload. Only the first `len` data bytes are valid. Frames
/// are at most 320x240, contain at most 16 objects, and never cross generations.
///
/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageChunk {
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub generation: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub frame_id: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub timestamp_us: u32,
    /// Zero with `object_count=0` means no object sample was acquired.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub objects_timestamp_us: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(4))]
    pub camera: Port,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(5))]
    pub width: u16,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(6))]
    pub height: u16,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(7))]
    pub object_count: u8,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(8))]
    pub offset: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(9))]
    pub len: u16,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(10))]
    #[cfg_attr(feature = "serde", serde(with = "serde_bytes"))]
    pub data: [u8; IMAGE_CHUNK_CAPACITY],
}

/// Since: Odyssey Desktop 0.7.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageObject {
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub generation: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub frame_id: u32,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub index: u8,
    /// Since: Odyssey Desktop 0.7.0.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub feature: ObjectFeature,
}

/// An owned, validated complete frame. Objects can be acquired before or after
///
/// the pixels: compare the wrapping device timestamps before drawing overlays.
/// No partially received image is exposed to host consumers.
///
/// Since: Odyssey Desktop 0.7.0.
#[cfg(feature = "std")]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct ImageFrame {
    /// Since: Odyssey Desktop 0.7.0.
    pub frame_id: u32,
    /// Since: Odyssey Desktop 0.7.0.
    pub timestamp_us: u32,
    /// Since: Odyssey Desktop 0.7.0.
    pub camera: Port,
    /// Since: Odyssey Desktop 0.7.0.
    pub width: u16,
    /// Since: Odyssey Desktop 0.7.0.
    pub height: u16,
    /// Since: Odyssey Desktop 0.7.0.
    pub pixels: std::vec::Vec<u8>,
    /// Since: Odyssey Desktop 0.7.0.
    pub objects: std::vec::Vec<ObjectFeature>,
    /// Since: Odyssey Desktop 0.7.0.
    pub objects_timestamp_us: u32,
}
