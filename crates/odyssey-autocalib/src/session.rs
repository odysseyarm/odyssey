//! Calibration workflow on top of a running camera.
//!
//! A [`Session`] owns the camera stream and walks through the steps the UI
//! shows: [`Session::preview`] while the user positions the camera,
//! [`Session::measure_screen`] once the pattern is on screen,
//! [`Session::preview_markers`] and [`Session::measure_markers`] with the
//! tracker markers switched on, and [`Session::finish`] for the result and the
//! data a review screen needs. The core returns data and enums only; the UI
//! turns [`Hint`]s and [`Step`]s into words.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::{GrayImage, RgbImage};
use nalgebra::{Matrix3, Point2};
use odyssey_common::{ScreenCalibration, MAX_SCREEN_ID};

use crate::camera::{CameraError, CameraGeometry, CameraStream, Frames};
use crate::imu::{GravityEstimator, Tilt};
use crate::markers::{self, MarkerBlob, MarkerError};
use crate::pattern::{detect, DetectFailure, Pattern};
use crate::plane::{apply_homography, project_pinhole, ransac_plane, RansacParams};
use crate::solve::{self, build_calibration, solve_screen, ScreenFit, SolveError, SolveOptions, BURST_FRAMES};

/// Infrared frames averaged when measuring markers.
pub const MARKER_FRAMES: usize = 5;
/// Frames discarded after changing infrared exposure so the sensor has
/// settled.
const SETTLE_FRAMES: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    /// Color and depth frames merged by [`Session::measure_screen`].
    pub burst_frames: usize,
    /// Infrared exposure for marker detection, microseconds. Short enough that
    /// only the marker LEDs stay bright.
    pub ir_exposure_us: u32,
    /// Physical screen diagonal in inches, when the user knows it. The result
    /// is rectified to this size and the screen's aspect ratio.
    pub screen_diagonal_in: Option<f64>,
    /// How long to wait for each frame.
    pub frame_timeout: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self { burst_frames: BURST_FRAMES, ir_exposure_us: 100, screen_diagonal_in: None, frame_timeout: Duration::from_secs(2) }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Camera(#[from] CameraError),
    #[error(transparent)]
    Solve(#[from] SolveError),
    #[error(transparent)]
    Markers(#[from] MarkerError),
    #[error("the screen has not been measured yet")]
    ScreenNotMeasured,
    #[error("the markers have not been measured yet")]
    MarkersNotMeasured,
}

/// The most important thing the user should change, or `Ready`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    Ready,
    /// The dot pattern is not visible.
    ScreenNotFound,
    /// Part of the screen is outside the image or hidden.
    ScreenPartlyVisible,
    TooDark,
    TooBright,
    TooClose,
    TooFar,
    /// The depth sensor returns too little usable data on the screen.
    DepthPoor,
    CameraTiltedTooMuch,
    /// The camera looks at the screen from too steep an angle.
    ScreenTooOblique,
}

/// One live frame with what the detector made of it.
#[derive(Debug, Clone)]
pub struct Preview {
    pub color: RgbImage,
    /// Centers of the detected dots in the color image.
    pub dots: Vec<Point2<f32>>,
    /// Screen corners (top left, top right, bottom right, bottom left) in the
    /// color image, when the pattern was found.
    pub screen_quad: Option<[Point2<f32>; 4]>,
    pub dots_matched: usize,
    pub dots_expected: usize,
    /// Share of the screen region with usable depth, `0..=1`.
    pub depth_coverage: Option<f32>,
    /// Distance from the camera to the screen in meters.
    pub distance_m: Option<f32>,
    pub tilt: Option<Tilt>,
    pub hint: Hint,
}

impl Preview {
    pub fn is_ready(&self) -> bool {
        self.hint == Hint::Ready
    }
}

/// Progress of a measurement, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Capturing { frame: usize, total: usize },
    Solving,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenSummary {
    /// Width and height in meters.
    pub size_m: [f64; 2],
    pub diagonal_in: f64,
    pub plane_rmse_m: f64,
    pub plane_max_error_m: f64,
    pub dots_used: usize,
    pub quad_px: [Point2<f32>; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerHint {
    Ready,
    /// No bright spots at all: markers off, or the infrared view is blocked.
    NoMarkers,
    TooFewMarkers { found: usize },
    /// Spots were found but too few sit where the markers belong.
    WrongPlace { matched: usize },
}

#[derive(Debug, Clone)]
pub struct MarkerPreview {
    pub ir: GrayImage,
    /// Bright spots in the infrared image.
    pub blobs: Vec<Point2<f32>>,
    /// Index into `blobs` for each of the six markers when they were matched.
    /// Matching needs a measured screen; without one `Ready` only means that
    /// at least six spots are visible.
    pub matched: Option<[usize; 6]>,
    pub hint: MarkerHint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkerSummary {
    /// Marker positions in meters in the screen frame.
    pub points_m: [Point2<f64>; 6],
    /// Largest distance between a marker and its layout position, meters.
    pub max_offset_m: f64,
}

/// Data for the review screen.
#[derive(Debug, Clone)]
pub struct Review {
    /// Color frame the pattern was measured in.
    pub color: RgbImage,
    pub plane_rmse_m: f64,
    pub plane_max_error_m: f64,
    pub plane_inliers: usize,
    pub screen_quad_px: [Point2<f32>; 4],
    /// Marker positions in the color image.
    pub marker_px: [Point2<f32>; 6],
    pub size_m: [f64; 2],
    pub diagonal_in: f64,
    pub dots_used: usize,
    pub dots_expected: usize,
    pub marker_max_offset_m: f64,
    /// Whether IMU data aligned the result to gravity.
    pub gravity_aligned: bool,
}

#[derive(Debug, Clone)]
pub struct Calibration {
    pub calibration: ScreenCalibration<f32>,
    pub review: Review,
}

struct ScreenMeasurement {
    fit: ScreenFit,
    color: RgbImage,
    gravity_aligned: bool,
}

struct MarkerMeasurement {
    summary: MarkerSummary,
    marker_px: [Point2<f32>; 6],
}

pub struct Session {
    stream: Box<dyn CameraStream>,
    geometry: CameraGeometry,
    pattern: Pattern,
    options: SessionOptions,
    gravity: GravityEstimator,
    screen: Option<ScreenMeasurement>,
    markers: Option<MarkerMeasurement>,
    marker_mode: bool,
}

fn pt32(p: Point2<f64>) -> Point2<f32> {
    Point2::new(p.x as f32, p.y as f32)
}

impl Session {
    /// Starts a session on an open camera. `aspect` is the screen's width over
    /// height, which fixes the pattern.
    pub fn start(mut stream: Box<dyn CameraStream>, aspect: f64, options: SessionOptions) -> Session {
        let geometry = *stream.geometry();
        // Depth quality depends on the projector, and a previous session may
        // have left the infrared sensor on a short exposure.
        let _ = stream.set_ir_auto_exposure();
        let _ = stream.set_projector_enabled(true);
        Session {
            stream,
            geometry,
            pattern: Pattern::new(aspect),
            options,
            gravity: GravityEstimator::new(),
            screen: None,
            markers: None,
            marker_mode: false,
        }
    }

    /// The pattern the screen must display.
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    pub fn geometry(&self) -> &CameraGeometry {
        &self.geometry
    }

    pub fn screen_fit(&self) -> Option<&ScreenFit> {
        self.screen.as_ref().map(|s| &s.fit)
    }

    pub fn tilt(&self) -> Option<Tilt> {
        self.gravity.tilt()
    }

    fn grab(&mut self) -> Result<Frames, SessionError> {
        let frames = self.stream.next_frames(self.options.frame_timeout)?;
        let imu = self.stream.drain_imu();
        self.gravity.extend(&imu);
        Ok(frames)
    }

    fn leave_marker_mode(&mut self) {
        if self.marker_mode {
            let _ = self.stream.set_ir_auto_exposure();
            let _ = self.stream.set_projector_enabled(true);
            self.marker_mode = false;
        }
    }

    fn enter_marker_mode(&mut self) -> Result<(), SessionError> {
        if self.marker_mode {
            return Ok(());
        }
        self.stream.set_ir_exposure_us(self.options.ir_exposure_us)?;
        // Cameras without a switchable projector report `Ok(false)`; the
        // projector's dots then stay below the relative threshold anyway.
        let _ = self.stream.set_projector_enabled(false)?;
        self.marker_mode = true;
        for _ in 0..SETTLE_FRAMES {
            self.grab()?;
        }
        Ok(())
    }

    /// Changes the infrared exposure used for marker detection. Takes effect
    /// at once while the camera is in marker mode and at the next entry
    /// otherwise, so a UI can offer an exposure slider next to the live marker
    /// preview.
    pub fn set_ir_exposure_us(&mut self, exposure_us: u32) -> Result<(), SessionError> {
        self.options.ir_exposure_us = exposure_us;
        if self.marker_mode {
            self.stream.set_ir_exposure_us(exposure_us)?;
            for _ in 0..SETTLE_FRAMES {
                self.grab()?;
            }
        }
        Ok(())
    }

    /// Grabs one frame and reports whether the screen is ready to measure.
    pub fn preview(&mut self) -> Result<Preview, SessionError> {
        self.leave_marker_mode();
        let frames = self.grab()?;
        let tilt = self.gravity.tilt();
        let expected = self.pattern.dots().len();
        let detection = detect(&self.pattern, &frames.color, &self.geometry.color);

        let mut preview = Preview {
            color: frames.color.clone(),
            dots: Vec::new(),
            screen_quad: None,
            dots_matched: 0,
            dots_expected: expected,
            depth_coverage: None,
            distance_m: None,
            tilt,
            hint: Hint::ScreenNotFound,
        };

        let detection = match detection {
            Ok(d) => d,
            Err(failure) => {
                preview.hint = hint_without_pattern(&frames.color, failure);
                return Ok(preview);
            }
        };
        preview.dots = detection.correspondences.iter().map(|c| pt32(c.raw)).collect();
        preview.screen_quad = Some(detection.quad_px.map(pt32));
        preview.dots_matched = detection.quality.matched;

        let depth = depth_stats(&self.geometry, &frames, &detection.homography);
        preview.depth_coverage = depth.coverage;
        preview.distance_m = depth.distance_m;

        preview.hint = if !region_inside_images(&self.geometry, &detection.homography) || detection.quality.completeness() < 0.8 {
            Hint::ScreenPartlyVisible
        } else if depth.distance_m.is_some_and(|d| d < MIN_DISTANCE_M) {
            Hint::TooClose
        } else if depth.distance_m.is_some_and(|d| d > MAX_DISTANCE_M) || detection.quality.spacing_px < MIN_SPACING_PX {
            Hint::TooFar
        } else if tilt.is_some_and(|t| t.pitch_deg.abs() > MAX_PITCH_DEG || t.roll_deg.abs() > MAX_ROLL_DEG) {
            Hint::CameraTiltedTooMuch
        } else if depth.cos_incidence.is_some_and(|c| c < MIN_INCIDENCE_COS) {
            Hint::ScreenTooOblique
        } else if depth.coverage.is_none_or(|c| c < MIN_DEPTH_COVERAGE) || depth.plane_rmse_m.is_some_and(|e| e > MAX_PREVIEW_RMSE_M) {
            Hint::DepthPoor
        } else {
            Hint::Ready
        };
        Ok(preview)
    }

    /// Captures a burst while the pattern is shown and measures the screen.
    pub fn measure_screen(&mut self, progress: &mut dyn FnMut(Step)) -> Result<ScreenSummary, SessionError> {
        self.leave_marker_mode();
        let total = self.options.burst_frames.max(1);
        let mut frames = Vec::with_capacity(total);
        for frame in 0..total {
            progress(Step::Capturing { frame, total });
            frames.push(self.grab()?);
        }
        progress(Step::Solving);
        let alignment = self.gravity.camera_to_aligned();
        let options = SolveOptions { screen_diagonal_in: self.options.screen_diagonal_in, ..SolveOptions::default() };
        let fit = solve_screen(&self.geometry, &self.pattern, &frames, &alignment, &options)?;
        let summary = ScreenSummary {
            size_m: fit.size_m,
            diagonal_in: fit.diagonal_in(),
            plane_rmse_m: fit.plane_rmse_m,
            plane_max_error_m: fit.plane_max_error_m,
            dots_used: fit.dots_used,
            quad_px: fit.quad_px.map(pt32),
        };
        let color = frames.pop().map(|f| f.color).unwrap_or_else(|| RgbImage::new(0, 0));
        self.screen = Some(ScreenMeasurement { fit, color, gravity_aligned: self.gravity.is_ready() });
        // A new screen measurement invalidates markers measured against the old one.
        self.markers = None;
        progress(Step::Done);
        Ok(summary)
    }

    /// Switches the camera to marker detection and reports what it sees.
    pub fn preview_markers(&mut self) -> Result<MarkerPreview, SessionError> {
        self.enter_marker_mode()?;
        let frames = self.grab()?;
        let blobs = markers::detect_blobs(&frames.ir);
        let mut preview = MarkerPreview {
            blobs: blobs.iter().map(|b| pt32(b.center)).collect(),
            ir: frames.ir,
            matched: None,
            hint: MarkerHint::Ready,
        };
        if blobs.is_empty() {
            preview.hint = MarkerHint::NoMarkers;
        } else if blobs.len() < 6 {
            preview.hint = MarkerHint::TooFewMarkers { found: blobs.len() };
        } else if let Some(screen) = &self.screen {
            match markers::locate(&self.geometry, &screen.fit, &blobs) {
                Ok(m) => preview.matched = Some(m.blob_for_marker),
                Err(MarkerError::LayoutMismatch { matched }) => preview.hint = MarkerHint::WrongPlace { matched },
                Err(MarkerError::TooFewBlobs { found }) => preview.hint = MarkerHint::TooFewMarkers { found },
            }
        }
        Ok(preview)
    }

    /// Measures the six markers from a short burst of infrared frames. Needs
    /// [`measure_screen`](Self::measure_screen) first.
    pub fn measure_markers(&mut self, progress: &mut dyn FnMut(Step)) -> Result<MarkerSummary, SessionError> {
        if self.screen.is_none() {
            return Err(SessionError::ScreenNotMeasured);
        }
        self.enter_marker_mode()?;
        let mut sums = [nalgebra::Vector2::<f64>::zeros(); 6];
        let mut max_offset = 0.0f64;
        let mut successes = 0usize;
        let mut last_error = None;
        let total = MARKER_FRAMES;
        for frame in 0..total {
            progress(Step::Capturing { frame, total });
            let frames = self.grab()?;
            let blobs: Vec<MarkerBlob> = markers::detect_blobs(&frames.ir);
            let screen = self.screen.as_ref().ok_or(SessionError::ScreenNotMeasured)?;
            match markers::locate(&self.geometry, &screen.fit, &blobs) {
                Ok(m) => {
                    successes += 1;
                    max_offset = max_offset.max(m.max_offset_m);
                    for (sum, p) in sums.iter_mut().zip(m.points_m) {
                        *sum += p.coords;
                    }
                }
                Err(e) => last_error = Some(e),
            }
        }
        progress(Step::Solving);
        if successes == 0 || successes * 2 < total {
            return Err(last_error.map(SessionError::Markers).unwrap_or(SessionError::Markers(MarkerError::TooFewBlobs { found: 0 })));
        }
        let points_m = sums.map(|s| Point2::from(s / successes as f64));
        let screen = self.screen.as_ref().ok_or(SessionError::ScreenNotMeasured)?;
        let marker_px = points_m.map(|p| screen.fit.screen_to_color_px(p).map(pt32).unwrap_or_else(Point2::origin));
        self.markers = Some(MarkerMeasurement { summary: MarkerSummary { points_m, max_offset_m: max_offset }, marker_px });
        progress(Step::Done);
        self.markers.as_ref().map(|m| m.summary).ok_or(SessionError::MarkersNotMeasured)
    }

    /// The calibration and review data, once the screen and markers are measured.
    pub fn finish(&self) -> Result<Calibration, SessionError> {
        let screen = self.screen.as_ref().ok_or(SessionError::ScreenNotMeasured)?;
        let markers = self.markers.as_ref().ok_or(SessionError::MarkersNotMeasured)?;
        let calibration = build_calibration(&screen.fit, &markers.summary.points_m);
        let review = Review {
            color: screen.color.clone(),
            plane_rmse_m: screen.fit.plane_rmse_m,
            plane_max_error_m: screen.fit.plane_max_error_m,
            plane_inliers: screen.fit.plane_inliers,
            screen_quad_px: screen.fit.quad_px.map(pt32),
            marker_px: markers.marker_px,
            size_m: screen.fit.size_m,
            diagonal_in: screen.fit.diagonal_in(),
            dots_used: screen.fit.dots_used,
            dots_expected: self.pattern.dots().len(),
            marker_max_offset_m: markers.summary.max_offset_m,
            gravity_aligned: screen.gravity_aligned,
        };
        Ok(Calibration { calibration, review })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.leave_marker_mode();
    }
}

const MIN_DISTANCE_M: f32 = 0.6;
const MAX_DISTANCE_M: f32 = 5.0;
/// Dots closer together than this many pixels are too small to localize well.
const MIN_SPACING_PX: f64 = 12.0;
const MAX_PITCH_DEG: f64 = 35.0;
const MAX_ROLL_DEG: f64 = 20.0;
/// Cosine of the largest angle between the screen normal and the line of
/// sight.
const MIN_INCIDENCE_COS: f64 = 0.35;
const MIN_DEPTH_COVERAGE: f32 = 0.6;
const MAX_PREVIEW_RMSE_M: f64 = 0.015;

#[derive(Default)]
struct DepthStats {
    coverage: Option<f32>,
    distance_m: Option<f32>,
    cos_incidence: Option<f64>,
    plane_rmse_m: Option<f64>,
}

/// Whether the inset screen region lies inside both the color and the depth
/// image.
fn region_inside_images(geometry: &CameraGeometry, unit_to_px: &Matrix3<f64>) -> bool {
    let (lo, hi) = solve::SCREEN_INSET;
    let color = &geometry.color;
    [(lo, lo), (hi, lo), (hi, hi), (lo, hi)].into_iter().all(|(u, v)| {
        apply_homography(unit_to_px, Point2::new(u, v)).is_some_and(|p| {
            p.x >= 0.0 && p.y >= 0.0 && p.x <= f64::from(color.width - 1) && p.y <= f64::from(color.height - 1)
        })
    })
}

fn hint_without_pattern(color: &RgbImage, failure: DetectFailure) -> Hint {
    let mut histogram = [0u32; 256];
    let mut saturated = 0u32;
    let mut count = 0u32;
    for p in color.pixels().step_by(7) {
        let luma = ((77 * u32::from(p[0]) + 150 * u32::from(p[1]) + 29 * u32::from(p[2])) >> 8) as usize;
        histogram[luma] += 1;
        count += 1;
        if luma >= 250 {
            saturated += 1;
        }
    }
    let p995 = {
        let target = (0.995 * f64::from(count)) as u32;
        let mut acc = 0;
        histogram
            .iter()
            .position(|&n| {
                acc += n;
                acc >= target
            })
            .unwrap_or(255)
    };
    if f64::from(saturated) > 0.15 * f64::from(count) {
        Hint::TooBright
    } else if p995 < 60 {
        Hint::TooDark
    } else if matches!(failure, DetectFailure::NoGrid { blobs } if blobs >= 20) {
        Hint::ScreenPartlyVisible
    } else {
        Hint::ScreenNotFound
    }
}

/// Distance, depth coverage and plane quality of the screen region in one
/// frame.
fn depth_stats(geometry: &CameraGeometry, frames: &Frames, unit_to_px: &Matrix3<f64>) -> DepthStats {
    let color_from_depth = geometry.color_to_depth.inverse();
    let points = solve::screen_depth_points(geometry, std::slice::from_ref(frames), unit_to_px, &color_from_depth);
    if points.is_empty() {
        return DepthStats { coverage: Some(0.0), ..DepthStats::default() };
    }

    let mut z: Vec<f64> = points.iter().map(|p| p.z).collect();
    z.sort_by(f64::total_cmp);
    let distance = z[z.len() / 2];

    // How many samples a fully valid region would yield at this distance.
    let (lo, hi) = solve::SCREEN_INSET;
    let depth_corners: Option<Vec<Point2<f64>>> = [(lo, lo), (hi, lo), (hi, hi), (lo, hi)]
        .into_iter()
        .map(|(u, v)| {
            let px = apply_homography(unit_to_px, Point2::new(u, v))?;
            let ray = nalgebra::Vector3::new(
                (px.x - geometry.color.cx) / geometry.color.fx,
                (px.y - geometry.color.cy) / geometry.color.fy,
                1.0,
            );
            project_pinhole(&geometry.depth, &geometry.color_to_depth.apply(&(ray * distance)))
        })
        .collect();
    let stride = f64::from((frames.depth.width() / 320).max(1));
    let coverage = depth_corners.map(|c| {
        let area: f64 = (0..4).map(|i| c[i].x * c[(i + 1) % 4].y - c[(i + 1) % 4].x * c[i].y).sum::<f64>().abs() / 2.0;
        ((points.len() as f64 * stride * stride) / area.max(1.0)).min(1.0) as f32
    });

    // A plane on a few hundred samples gives the viewing angle and a noise
    // estimate without the full burst.
    let step = (points.len() / 1500).max(1);
    let subset: Vec<_> = points.iter().step_by(step).copied().collect();
    let fit = ransac_plane(&subset, &RansacParams { max_trials: 300, score_points: 1500, ..RansacParams::default() });
    DepthStats {
        coverage,
        distance_m: Some(distance as f32),
        cos_incidence: fit.as_ref().map(|f| f.plane.normal.dot(&f.plane.point.normalize())),
        plane_rmse_m: fit.as_ref().map(|f| f.rmse),
    }
}

/// File name the hub reads for screen `id`.
pub fn screen_file_name(id: u8) -> String {
    format!("screen_{id}.json")
}

/// Serializes a calibration the way the hub's loader expects: compact JSON
/// with `rotation` as `[i, j, k, w]`, `homography` as nine column-major
/// values and `object_points` as six `[x, y, z]` triples.
pub fn calibration_to_json(calibration: &ScreenCalibration<f32>) -> Result<String, serde_json::Error> {
    serde_json::to_string(calibration)
}

pub fn calibration_from_json(json: &str) -> Result<ScreenCalibration<f32>, serde_json::Error> {
    serde_json::from_str(json)
}

fn check_id(id: u8) -> io::Result<()> {
    if id > MAX_SCREEN_ID {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("screen id {id} is above {MAX_SCREEN_ID}")));
    }
    Ok(())
}

/// Writes `screen_<id>.json` into `screens_dir`, creating the directory. The
/// write goes through a temporary file so the hub never reads a partial
/// calibration. Returns the file's path.
pub fn save_calibration(screens_dir: &Path, id: u8, calibration: &ScreenCalibration<f32>) -> io::Result<PathBuf> {
    check_id(id)?;
    std::fs::create_dir_all(screens_dir)?;
    let path = screens_dir.join(screen_file_name(id));
    let temp = screens_dir.join(format!(".{}.tmp", screen_file_name(id)));
    let json = calibration_to_json(calibration).map_err(io::Error::other)?;
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &path)?;
    Ok(path)
}

pub fn load_calibration(screens_dir: &Path, id: u8) -> io::Result<ScreenCalibration<f32>> {
    check_id(id)?;
    let json = std::fs::read_to_string(screens_dir.join(screen_file_name(id)))?;
    calibration_from_json(&json).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::{Matrix3, Point3, UnitQuaternion};

    fn sample() -> ScreenCalibration<f32> {
        ScreenCalibration {
            rotation: UnitQuaternion::from_euler_angles(0.1, -0.2, 0.05),
            homography: Matrix3::new(0.82, 0.0, 0.0, 0.0, 1.46, 0.0, 0.0, 0.0, 1.0),
            object_points: [
                Point3::new(0.21, 0.2, 0.0),
                Point3::new(0.18, 0.56, 0.0),
                Point3::new(0.93, 0.55, 0.0),
                Point3::new(0.62, 0.24, 0.0),
                Point3::new(0.96, 0.17, 0.0),
                Point3::new(0.6, 0.52, 0.0),
            ],
        }
    }

    #[test]
    fn json_round_trip_keeps_corners_and_markers() {
        let cal = sample();
        let json = calibration_to_json(&cal).unwrap();
        let back = calibration_from_json(&json).unwrap();
        assert_eq!(back.homography, cal.homography);
        assert_eq!(back.object_points, cal.object_points);
        assert!((back.rotation.angle_to(&cal.rotation)).abs() < 1e-6);
        let expected = [Point2::new(0.0, 0.0), Point2::new(1.0 / 0.82, 0.0), Point2::new(0.0, 1.0 / 1.46), Point2::new(1.0 / 0.82, 1.0 / 1.46)];
        for (got, want) in back.bounds().iter().zip(expected) {
            assert!((got - want).norm() < 1e-6, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn json_layout_matches_the_hub_schema() {
        let value: serde_json::Value = serde_json::from_str(&calibration_to_json(&sample()).unwrap()).unwrap();
        assert_eq!(value["rotation"].as_array().unwrap().len(), 4);
        assert_eq!(value["homography"].as_array().unwrap().len(), 9);
        assert_eq!(value["object_points"].as_array().unwrap().len(), 6);
        // Column-major: the second entry is the (row 1, col 0) element, zero here,
        // and the 5th entry is the (1, 1) element.
        assert!((value["homography"][4].as_f64().unwrap() - 1.46).abs() < 1e-6);
        assert!((value["homography"][0].as_f64().unwrap() - 0.82).abs() < 1e-6);
    }

    #[test]
    fn save_and_load_use_the_hub_file_name() {
        let dir = std::env::temp_dir().join(format!("odyssey-autocalib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_calibration(&dir.join("screens"), 3, &sample()).unwrap();
        assert_eq!(path.file_name().unwrap(), "screen_3.json");
        let back = load_calibration(&dir.join("screens"), 3).unwrap();
        assert_eq!(back.object_points, sample().object_points);
        assert!(save_calibration(&dir, MAX_SCREEN_ID + 1, &sample()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
