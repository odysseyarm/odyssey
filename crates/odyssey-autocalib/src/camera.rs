//! Camera-independent description of what the calibration needs from a depth
//! camera. Backends translate their SDK into these types; nothing else in the
//! crate depends on an SDK.
//!
//! Units: lengths are meters, pixel coordinates have the origin at the top left
//! of the image with `x` right and `y` down, camera frames are right-handed
//! with `+z` forward, `+x` right and `+y` down (the usual pinhole convention).

use std::time::Duration;

use image::{GrayImage, ImageBuffer, Luma, RgbImage};
use nalgebra::{Matrix3, Vector3};

pub type DepthImage = ImageBuffer<Luma<u16>, Vec<u16>>;

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("no depth camera found")]
    NoDevice,
    #[error("camera does not provide {0}")]
    Unsupported(&'static str),
    #[error("camera did not deliver a frame in time")]
    Timeout,
    #[error("camera SDK error: {0}")]
    Sdk(String),
}

/// One camera attached to the machine, as listed before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// Backend-specific identifier that [`CameraBackend::open`] accepts.
    pub id: String,
    /// Human-readable model name, e.g. "Gemini 335".
    pub name: String,
    pub serial: String,
}

/// Lists and opens cameras of one kind. Listing is cheap and holds no device
/// open; a camera only streams while the [`CameraStream`] returned by `open`
/// is alive.
pub trait CameraBackend {
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError>;
    fn open(&self, camera: &CameraInfo, settings: &CaptureSettings) -> Result<Box<dyn CameraStream>, CameraError>;
}

/// Settings applied when a stream starts. The calibration UI exposes these as
/// controls and a stream restarts to change them.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureSettings {
    /// Color exposure in microseconds. Fixed rather than automatic so the
    /// projected dots do not bloom when the screen is brighter than the room.
    pub color_exposure_us: u32,
    pub color_gain: u32,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self { color_exposure_us: 1500, color_gain: 1 }
    }
}

/// A running camera. Dropping it stops streaming and releases the device.
pub trait CameraStream: Send {
    /// Intrinsics and extrinsics. Stable for the life of the stream.
    fn geometry(&self) -> &CameraGeometry;

    /// Next time-synchronized color, depth and infrared frames, or
    /// `Err(CameraError::Timeout)` if none arrived within `timeout`.
    fn next_frames(&mut self, timeout: Duration) -> Result<Frames, CameraError>;

    /// Inertial samples received since the last call, oldest first. Empty if
    /// the camera has no IMU.
    fn drain_imu(&mut self) -> Vec<ImuSample>;

    /// Sets infrared exposure in microseconds, switching auto exposure off.
    /// Marker detection runs with a short exposure so only the LEDs stay bright.
    fn set_ir_exposure_us(&mut self, exposure_us: u32) -> Result<(), CameraError>;

    /// Restores infrared auto exposure.
    fn set_ir_auto_exposure(&mut self) -> Result<(), CameraError>;

    /// Turns the depth projector on or off. Marker detection switches it off
    /// when the camera allows, because its dot pattern shows up in the infrared
    /// image. Returns `Ok(false)` if the camera has no switchable projector.
    fn set_projector_enabled(&mut self, enabled: bool) -> Result<bool, CameraError>;
}

/// Pinhole intrinsics with Brown-Conrady distortion, as OpenCV and the Orbbec
/// SDK define it: `k1..k3` and `k4..k6` are the radial numerator and
/// denominator terms, `p1` and `p2` the tangential terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intrinsics {
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub k: [f64; 6],
    pub p: [f64; 2],
}

/// Rigid transform taking points from one camera's frame to another's:
/// `p_to = rotation * p_from + translation`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extrinsics {
    pub rotation: Matrix3<f64>,
    pub translation: Vector3<f64>,
}

impl Extrinsics {
    pub fn identity() -> Self {
        Self { rotation: Matrix3::identity(), translation: Vector3::zeros() }
    }

    pub fn inverse(&self) -> Self {
        let rotation = self.rotation.transpose();
        Self { rotation, translation: -(rotation * self.translation) }
    }

    pub fn apply(&self, p: &Vector3<f64>) -> Vector3<f64> {
        self.rotation * p + self.translation
    }
}

/// Everything the solver needs to relate the three image streams.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraGeometry {
    pub color: Intrinsics,
    pub depth: Intrinsics,
    /// Left infrared imager. Equals `depth` on cameras whose depth is computed
    /// in the left infrared frame.
    pub ir: Intrinsics,
    /// Takes points from the color camera's frame to the depth camera's frame.
    pub color_to_depth: Extrinsics,
    /// Takes points from the infrared camera's frame to the depth camera's
    /// frame. Identity when the two share a frame.
    pub ir_to_depth: Extrinsics,
}

/// One synchronized capture. The streams are not pixel-aligned; the solver
/// maps between them with [`CameraGeometry`].
#[derive(Debug, Clone)]
pub struct Frames {
    pub color: RgbImage,
    /// Raw depth in sensor units; multiply by `depth_unit_m` for meters.
    /// Zero marks an invalid pixel.
    pub depth: DepthImage,
    pub depth_unit_m: f32,
    pub ir: GrayImage,
}

impl Frames {
    /// Depth at a pixel in meters, or `None` outside the image or where the
    /// sensor reported no measurement.
    pub fn depth_m(&self, x: u32, y: u32) -> Option<f64> {
        if x >= self.depth.width() || y >= self.depth.height() {
            return None;
        }
        match self.depth.get_pixel(x, y).0[0] {
            0 => None,
            raw => Some(f64::from(raw) * f64::from(self.depth_unit_m)),
        }
    }
}

/// One inertial reading, in the camera's frame. Timestamps share an arbitrary
/// epoch; only their differences matter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuSample {
    pub timestamp: Duration,
    /// Angular rate in rad/s.
    pub gyro: Vector3<f64>,
    /// Specific force in m/s^2; reads `-gravity` when the camera is at rest.
    pub accel: Vector3<f64>,
}

// Backends express `ImuSample` vectors in the color camera's frame, not the
// IMU's own, so the solver never needs the IMU extrinsics.
