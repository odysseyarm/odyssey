//! Reusable native-pixel pattern detection and camera calibration.
//!
//! Live USB capture and its lifecycle belong to odyssey-vision-tool. The remaining
//! auxiliary binaries inspect image files; this library never opens a serial port
//! or changes device firmware. Solvers return compatible OpenCV JSON rather than
//! writing to a process-global calibration directory.

pub mod aprilgrid;
pub mod calibration;
pub mod charuco;
pub mod chessboard;
pub mod circles;
pub mod mot_data;
pub mod utils;

/// Typed failures shared by the image-file tools and integrated calibration UI.
#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("image I/O failed")]
    Io(#[from] std::io::Error),
    #[error("image encoding failed")]
    Image(#[from] image::ImageError),
    #[error("OpenCV operation failed")]
    OpenCv(#[from] opencv::Error),
    #[error("camera calibration failed")]
    Calibration(#[from] ccalib::CalibrationError),
    #[error("image buffer does not match its dimensions")]
    InvalidImageBuffer,
    #[error("unable to open calibration file {path}")]
    CalibrationFile { path: String },
    #[error("invalid calibration input: {0}")]
    InvalidCalibration(&'static str),
    #[error("calibration cancelled")]
    Cancelled,
    #[error("calibration JSON failed")]
    Json(#[from] serde_json::Error),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Port {
    Wf,
    Nf,
}

pub struct PatternPoints {
    pub points: opencv::core::Vector<opencv::core::Point2f>,
    pub pattern_found: bool,
}
