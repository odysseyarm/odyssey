//! USB-only grayscale capture (modern: reboot-free; legacy: reboot to resume tracking).
//!
//! Start reserves one request id: its status, optional objects and pixel chunks all
//! use that id. Object samples, when present, precede pixels and are indexed from
//! zero. Image-only capture sends zero object count and timestamp. Every pixel chunk
//! repeats the complete frame header; offsets are contiguous byte offsets in a
//! row-major, native-camera-coordinate grayscale image. Stop uses a separate
//! request id. A stale Stop acknowledges its generation without stopping a newer
//! capture. Generations isolate cancelled sessions, including reused request ids.

use odyssey_protocol_macros::protocol_type;

use crate::Port;

pub const IMAGE_CHUNK_CAPACITY: usize = 192;
pub const MAX_IMAGE_OBJECTS: usize = 16;
pub const MAX_IMAGE_PIXELS: usize = 320 * 240;
/// Modern firmware ends capture and restores tracking when no same-generation
/// Renew arrives within this window. Start grants the first lease.
pub const IMAGE_LEASE_TIMEOUT_MS: u64 = 5000;
/// Viewer renewal cadence; leaves several missed renewals before expiry.
pub const IMAGE_LEASE_RENEW_INTERVAL_MS: u64 = 1000;

/// One camera at a time, requesting 1..=15 FPS. Acquisition and USB backpressure
/// can lower the delivered rate. Not supported over BLE.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageStreamConfig {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub camera: Port,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub fps: u8,
}

/// Sensor-reported diagnostics, not additional tracking acceptance criteria.
/// Coordinates are native image pixels, not the normalized tracking grid.
/// `bounds`, when available, is [left, top, right, bottom]. Missing sensor fields
/// must be None rather than invented values.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectFeature {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub id: u8,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub x: f32,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub y: f32,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub area: u32,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub mean_brightness: u8,
    #[cfg_attr(feature = "minicbor", n(5))]
    pub max_brightness: Option<u8>,
    #[cfg_attr(feature = "minicbor", n(6))]
    pub bounds: Option<[u16; 4]>,
    #[cfg_attr(feature = "minicbor", n(7))]
    pub radius: Option<f32>,
    #[cfg_attr(feature = "minicbor", n(8))]
    pub aspect_ratio: Option<f32>,
    #[cfg_attr(feature = "minicbor", n(9))]
    pub is_circle: Option<bool>,
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageCommand {
    #[cfg_attr(feature = "minicbor", n(0))]
    Start {
        #[cfg_attr(feature = "minicbor", n(0))]
        generation: u32,
        #[cfg_attr(feature = "minicbor", n(1))]
        config: ImageStreamConfig,
    },
    #[cfg_attr(feature = "minicbor", n(1))]
    Stop {
        #[cfg_attr(feature = "minicbor", n(0))]
        generation: u32,
    },
    /// Extends the active session's lease. Never answered; stale generations are ignored.
    #[cfg_attr(feature = "minicbor", n(2))]
    Renew {
        #[cfg_attr(feature = "minicbor", n(0))]
        generation: u32,
    },
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(eq, eq_int))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageStatusCode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Started,
    #[cfg_attr(feature = "minicbor", n(1))]
    Stopped,
    #[cfg_attr(feature = "minicbor", n(2))]
    UnsupportedCamera,
    #[cfg_attr(feature = "minicbor", n(3))]
    UsbRequired,
    #[cfg_attr(feature = "minicbor", n(4))]
    Busy,
    #[cfg_attr(feature = "minicbor", n(5))]
    CaptureError,
    /// Firmware does not implement image capture.
    #[cfg_attr(feature = "minicbor", n(6))]
    Unsupported,
    /// Pixel transfer ended, but legacy sensor image mode requires reboot/reconnect.
    #[cfg_attr(feature = "minicbor", n(7))]
    RebootRequired,
    /// No Renew arrived in time; modern firmware restored tracking.
    #[cfg_attr(feature = "minicbor", n(8))]
    LeaseExpired,
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageStatus {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub generation: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub status: ImageStatusCode,
}

/// A bounded pixel payload. Only the first `len` data bytes are valid. Frames
/// are at most 320x240, contain at most 16 objects, and never cross generations.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageChunk {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub generation: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub frame_id: u32,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub timestamp_us: u32,
    /// Zero with object_count=0 means no object sample was acquired.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub objects_timestamp_us: u32,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub camera: Port,
    #[cfg_attr(feature = "minicbor", n(5))]
    pub width: u16,
    #[cfg_attr(feature = "minicbor", n(6))]
    pub height: u16,
    #[cfg_attr(feature = "minicbor", n(7))]
    pub object_count: u8,
    #[cfg_attr(feature = "minicbor", n(8))]
    pub offset: u32,
    #[cfg_attr(feature = "minicbor", n(9))]
    pub len: u16,
    #[cfg_attr(feature = "minicbor", n(10))]
    #[cfg_attr(feature = "serde", serde(with = "serde_bytes"))]
    pub data: [u8; IMAGE_CHUNK_CAPACITY],
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageObject {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub generation: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub frame_id: u32,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub index: u8,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub feature: ObjectFeature,
}

/// An owned, validated complete frame. Objects can be acquired before or after
/// the pixels: compare the wrapping device timestamps before drawing overlays.
/// No partially received image is exposed to host consumers.
#[cfg(feature = "std")]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct ImageFrame {
    pub frame_id: u32,
    pub timestamp_us: u32,
    pub camera: Port,
    pub width: u16,
    pub height: u16,
    pub pixels: std::vec::Vec<u8>,
    pub objects: std::vec::Vec<ObjectFeature>,
    pub objects_timestamp_us: u32,
}
