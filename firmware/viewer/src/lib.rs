//! Reusable native-pixel pattern detection and camera calibration.
//!
//! Live USB capture and its lifecycle belong to odyssey-vision-tool. The other
//! binaries inspect image files. This library never opens a serial port or
//! changes device firmware. Solvers return OpenCV-compatible JSON instead of
//! writing to a process-global calibration directory.

/// Since: Odyssey Desktop 0.6.0.
pub mod aprilgrid;
/// Since: Odyssey Desktop 0.7.0.
pub mod calibration;
/// Since: Odyssey Desktop 0.6.0.
pub mod charuco;
/// Since: Odyssey Desktop 0.6.0.
pub mod chessboard;
/// Since: Odyssey Desktop 0.6.0.
pub mod circles;
/// Since: Odyssey Desktop 0.6.0.
pub mod mot_data;
/// Since: Odyssey Desktop 0.6.0.
pub mod utils;

/// Typed failures shared by the image-file tools and integrated calibration UI.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    /// Since: Odyssey Desktop 0.7.0.
    #[error("image I/O failed")]
    Io(#[from] std::io::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("image encoding failed")]
    Image(#[from] image::ImageError),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("OpenCV operation failed")]
    OpenCv(#[from] opencv::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("camera calibration failed")]
    Calibration(#[from] ccalib::CalibrationError),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("image buffer does not match its dimensions")]
    InvalidImageBuffer,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("unable to open calibration file {path}")]
    CalibrationFile {
        #[doc = "Since: Odyssey Desktop 0.7.0."]
        path: String,
    },
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid calibration input: {0}")]
    InvalidCalibration(&'static str),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("calibration cancelled")]
    Cancelled,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("calibration JSON failed")]
    Json(#[from] serde_json::Error),
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Port {
    /// Since: Odyssey Desktop 0.6.0.
    Wf,
    /// Since: Odyssey Desktop 0.6.0.
    Nf,
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug)]
pub struct PatternPoints {
    /// Since: Odyssey Desktop 0.7.0.
    pub points: opencv::core::Vector<opencv::core::Point2f>,
    /// Since: Odyssey Desktop 0.7.0.
    pub pattern_found: bool,
}
