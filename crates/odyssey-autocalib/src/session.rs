//! Calibration workflow on top of a running camera.
//!
//! A [`Session`] owns the camera stream and walks through the steps the UI
//! shows: [`Session::preview`] while the user positions the camera,
//! [`Session::measure_screen`] once the pattern is on screen,
//! [`Session::preview_markers`] and [`Session::measure_markers`] with the
//! tracker markers switched on, and [`Session::finish`] for the result and the
//! data a review screen needs. The core returns data and enums only; the UI
//! turns [`Hint`]s and [`Step`]s into words.

use std::fmt;
use std::time::{Duration, Instant};

use image::{GrayImage, RgbImage};
use nalgebra::{Point2, Vector2};
use odyssey_common::ScreenCalibration;

use crate::camera::{CameraError, CameraGeometry, CameraStream, Frames};
use crate::imu::{GravityEstimator, Tilt};
use crate::markers::{self, MarkerBlob, MarkerError, MarkerFit};
pub use crate::pattern::{EdgeCheck, SideSet};
use crate::pattern::{Pattern, check_edges, detect};
pub use crate::placement::Circle;
use crate::solve::{
    self, BURST_FRAMES, ScreenFit, SolveError, SolveOptions, build_calibration, solve_screen,
};
pub use crate::solve::{KnownSize, SizeCheck, SizeSource};

mod analysis;
mod storage;

pub use storage::{
    calibration_from_json, calibration_to_json, load_calibration, save_calibration,
    screen_file_name,
};

/// Infrared frames averaged when measuring markers.
pub const MARKER_FRAMES: usize = 5;

/// Frames discarded after changing infrared exposure so the sensor has
/// settled.
const SETTLE_FRAMES: usize = 4;

/// How many markers the tracker needs: the length of the marker arrays below.
const MARKER_COUNT: usize = 6;

/// Least time between two logged accounts of a failing marker search.
const MARKER_REPORT_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    /// Color and depth frames merged by [`Session::measure_screen`].
    pub burst_frames: usize,
    /// Infrared exposure for marker detection, microseconds. Short enough that
    /// only the marker LEDs stay bright.
    pub ir_exposure_us: u32,
    /// Declared screen size, from the display's EDID or the user. The result
    /// is rectified to it, which removes depth scale error. Leave unset for a
    /// projector, whose size only the depth camera can tell.
    pub known_size: Option<KnownSize>,
    /// Refuse to call the screen ready, and to measure it, while the display
    /// crops the pattern's border. Set it together with a `known_size` taken
    /// from the panel: an overscanned image is larger than the panel, so the
    /// panel size is no scale anchor for it.
    pub require_visible_edges: bool,
    /// How long to wait for each frame.
    pub frame_timeout: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            burst_frames: BURST_FRAMES,
            ir_exposure_us: 100,
            known_size: None,
            require_visible_edges: false,
            frame_timeout: Duration::from_secs(2),
        }
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
    /// The display crops the pattern's border on these sides and the options
    /// ask for the whole image to be visible.
    #[error("the screen does not show the whole picture; its border is cut off")]
    EdgesCropped(SideSet),
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
    /// The display cuts off the picture's border (overscan), so its panel
    /// size no longer matches the pattern. Reported only when
    /// [`SessionOptions::require_visible_edges`] is set.
    EdgesCropped,
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
    /// Whether the pattern's border ring is visible: `Cropped` means the
    /// display shows less than the whole image (overscan). Only blocks
    /// [`Hint::Ready`] when [`SessionOptions::require_visible_edges`] is set.
    pub edges: EdgeCheck,
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
    pub size_source: SizeSource,
    pub size_check: Option<SizeCheck>,
    pub edges: EdgeCheck,
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
    TooFewMarkers {
        found: usize,
    },
    /// Not six spots on or near the screen; the markers cannot be told from other lights.
    WrongCount {
        near_screen: usize,
    },
    /// Six spots on the screen, but no three of them make the row the tracker needs.
    NoRow,
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
    /// Marker positions in meters in the screen frame, in the order of
    /// `ScreenCalibration::object_points`.
    pub points_m: [Point2<f64>; 6],
    /// See [`Review::seed_circle`].
    pub seed_circle: Option<Circle>,
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
    /// Where `size_m` comes from.
    pub size_source: SizeSource,
    /// The depth-measured size against the declared one, when one was declared.
    /// Check [`SizeCheck::disagrees`] to warn about overscan or a wrong size.
    pub size_check: Option<SizeCheck>,
    /// Whether the pattern's border was visible when the screen was measured.
    pub edges: EdgeCheck,
    pub dots_used: usize,
    pub dots_expected: usize,
    /// The circle through the first three markers, where the tracker's seed solve is
    /// ill-conditioned for a camera whose position, projected onto the screen's plane, is on
    /// it. `None` when the three are collinear.
    pub seed_circle: Option<Circle>,
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
    edges: EdgeCheck,
    gravity_aligned: bool,
    /// The burst the fit came from, kept so the same measurement can be solved
    /// again under a different size anchor without the camera.
    frames: Vec<Frames>,
}

struct MarkerMeasurement {
    summary: MarkerSummary,
    marker_px: [Point2<f32>; 6],
    /// Blobs of each measured frame, kept for the same reason as the screen's
    /// frames: markers are located against a fit, and a new fit moves them.
    bursts: Vec<Vec<MarkerBlob>>,
}

/// Locates the markers in every burst frame against `fit` and averages the
/// frames that matched.
fn average_markers(
    geometry: &CameraGeometry,
    fit: &ScreenFit,
    bursts: &[Vec<MarkerBlob>],
) -> Result<(MarkerSummary, [Point2<f32>; 6]), SessionError> {
    let mut reference: Option<MarkerFit> = None;
    let mut sums = [Vector2::<f64>::zeros(); MARKER_COUNT];
    let mut successes = 0usize;
    let mut last_error = None;
    for blobs in bursts {
        let located = match markers::locate(geometry, fit, blobs) {
            Ok(located) => located,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        // Each frame is ordered on its own, and the markers do not move, so the
        // later frames take the labels of the first one.
        let points = match &reference {
            None => located.points_m,
            Some(first) => {
                let Some(points) = markers::relabel(&first.points_m, &located.points_m) else {
                    continue;
                };
                points
            }
        };
        successes += 1;
        for (sum, p) in sums.iter_mut().zip(points) {
            *sum += p.coords;
        }
        reference.get_or_insert(located);
    }
    let Some(first) = reference.filter(|_| solve::is_at_least_half(successes, bursts.len())) else {
        return Err(SessionError::Markers(
            last_error.unwrap_or(MarkerError::TooFewBlobs { found: 0 }),
        ));
    };
    let points_m = sums.map(|s| Point2::from(s / successes as f64));
    let marker_px = points_m.map(|p| {
        fit.screen_to_color_px(p)
            .map(pt32)
            .unwrap_or_else(Point2::origin)
    });
    Ok((
        MarkerSummary {
            points_m,
            seed_circle: first.seed_circle,
        },
        marker_px,
    ))
}

/// What an attempt looked at: the frames the solver and the marker search used.
#[derive(Debug, Clone)]
pub struct AttemptFrames {
    pub geometry: CameraGeometry,
    /// Color, depth and infrared of every frame in the screen burst.
    pub screen_burst: Vec<Frames>,
    /// The infrared frames of the latest marker measurement.
    pub marker_burst_ir: Vec<GrayImage>,
    /// The latest infrared frame shown while positioning for the markers.
    pub marker_preview_ir: Option<GrayImage>,
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
    /// The burst the last failed screen measurement was made from.
    failed_burst: Option<Vec<Frames>>,
    /// The infrared frames of the latest marker measurement, and the latest preview.
    marker_burst_ir: Vec<GrayImage>,
    marker_preview_ir: Option<GrayImage>,
    marker_report_at: Option<Instant>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("options", &self.options)
            .field("screen_measured", &self.screen.is_some())
            .field("markers_measured", &self.markers.is_some())
            .field("marker_mode", &self.marker_mode)
            .finish_non_exhaustive()
    }
}

fn pt32(p: Point2<f64>) -> Point2<f32> {
    Point2::new(p.x as f32, p.y as f32)
}

fn screen_summary(fit: &ScreenFit, edges: EdgeCheck) -> ScreenSummary {
    ScreenSummary {
        size_m: fit.size_m,
        diagonal_in: fit.diagonal_in(),
        size_source: fit.size_source,
        size_check: fit.size_check,
        edges,
        plane_rmse_m: fit.plane_rmse_m,
        plane_max_error_m: fit.plane_max_error_m,
        dots_used: fit.dots_used,
        quad_px: fit.quad_px.map(pt32),
    }
}

/// What a failed marker search says to the user.
fn marker_hint(error: &MarkerError) -> MarkerHint {
    match *error {
        MarkerError::TooFewBlobs { found } => MarkerHint::TooFewMarkers { found },
        MarkerError::WrongCount { near_screen } => MarkerHint::WrongCount { near_screen },
        MarkerError::Placement(_) => MarkerHint::NoRow,
    }
}

/// Logs a failed marker search with where the spots landed on the screen, at
/// most once per [`MARKER_REPORT_INTERVAL`]. It goes to the app's log (numbers
/// only, no images), which the desktop app also keeps as a file.
fn report_marker_failure(
    last_report: &mut Option<Instant>,
    geometry: &CameraGeometry,
    fit: &ScreenFit,
    blobs: &[MarkerBlob],
    error: &MarkerError,
) {
    let due = last_report.is_none_or(|t| t.elapsed() > MARKER_REPORT_INTERVAL);
    if due && !matches!(error, MarkerError::TooFewBlobs { .. }) {
        *last_report = Some(Instant::now());
        tracing::info!(
            "marker search failed ({error}):\n{}",
            markers::describe_spots(geometry, fit, blobs)
        );
    }
}

impl Session {
    /// Starts a session on an open camera. `aspect` is the screen's width over
    /// height, which fixes the pattern.
    pub fn start(mut stream: Box<dyn CameraStream>, aspect: f64, options: SessionOptions) -> Self {
        let geometry = *stream.geometry();
        // Depth quality depends on the projector, and a previous session may
        // have left the infrared sensor on a short exposure.
        let _ = stream.set_ir_auto_exposure();
        let _ = stream.set_projector_enabled(true);
        Self {
            stream,
            geometry,
            pattern: Pattern::new(aspect),
            options,
            gravity: GravityEstimator::new(),
            screen: None,
            markers: None,
            marker_mode: false,
            failed_burst: None,
            marker_burst_ir: Vec::new(),
            marker_preview_ir: None,
            marker_report_at: None,
        }
    }

    /// The pattern the screen must display.
    pub fn pattern(&self) -> &Pattern {
        &self.pattern
    }

    /// Everything the attempt looked at, for a diagnostic bundle: the screen burst
    /// (the one that failed, else the one the screen was measured from) and the
    /// infrared frames of the latest marker measurement and preview.
    pub fn attempt_frames(&self) -> AttemptFrames {
        AttemptFrames {
            geometry: self.geometry,
            screen_burst: self
                .failed_burst
                .clone()
                .or_else(|| self.screen.as_ref().map(|s| s.frames.clone()))
                .unwrap_or_default(),
            marker_burst_ir: self.marker_burst_ir.clone(),
            marker_preview_ir: self.marker_preview_ir.clone(),
        }
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

    /// Grabs `total` frames, announcing each as [`Step::Capturing`] before it
    /// is grabbed, and keeps what `keep` makes of every one.
    fn grab_burst<T>(
        &mut self,
        total: usize,
        progress: &mut dyn FnMut(Step),
        mut keep: impl FnMut(Frames) -> T,
    ) -> Result<Vec<T>, SessionError> {
        let mut kept = Vec::with_capacity(total);
        for frame in 0..total {
            progress(Step::Capturing { frame, total });
            kept.push(keep(self.grab()?));
        }
        Ok(kept)
    }

    /// Discards frames so the sensor settles after an exposure change.
    fn settle(&mut self) -> Result<(), SessionError> {
        for _ in 0..SETTLE_FRAMES {
            self.grab()?;
        }
        Ok(())
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
        self.stream
            .set_ir_exposure_us(self.options.ir_exposure_us)?;
        // Cameras without a switchable projector report `Ok(false)`; the
        // projector's dots then stay below the relative threshold anyway.
        let _ = self.stream.set_projector_enabled(false)?;
        self.marker_mode = true;
        self.settle()
    }

    /// Changes the infrared exposure used for marker detection. Takes effect
    /// at once while the camera is in marker mode and at the next entry
    /// otherwise, so a UI can offer an exposure slider next to the live marker
    /// preview.
    pub fn set_ir_exposure_us(&mut self, exposure_us: u32) -> Result<(), SessionError> {
        self.options.ir_exposure_us = exposure_us;
        if self.marker_mode {
            self.stream.set_ir_exposure_us(exposure_us)?;
            self.settle()?;
        }
        Ok(())
    }

    /// Grabs one frame and reports whether the screen is ready to measure.
    pub fn preview(&mut self) -> Result<Preview, SessionError> {
        self.leave_marker_mode();
        let frames = self.grab()?;
        let tilt = self.gravity.tilt();
        let detection = detect(&self.pattern, &frames.color, &self.geometry.color);

        let mut preview = Preview {
            color: frames.color.clone(),
            dots: Vec::new(),
            screen_quad: None,
            dots_matched: 0,
            dots_expected: self.pattern.dots().len(),
            depth_coverage: None,
            distance_m: None,
            tilt,
            edges: EdgeCheck::Unknown,
            hint: Hint::ScreenNotFound,
        };

        let detection = match detection {
            Ok(d) => d,
            Err(failure) => {
                preview.hint = analysis::hint_without_pattern(&frames.color, failure);
                return Ok(preview);
            }
        };
        preview.dots = detection
            .correspondences
            .iter()
            .map(|c| pt32(c.raw))
            .collect();
        preview.screen_quad = Some(detection.quad_px.map(pt32));
        preview.dots_matched = detection.quality.matched;

        let depth = analysis::depth_stats(&self.geometry, &frames, &detection.homography);
        preview.depth_coverage = depth.coverage;
        preview.distance_m = depth.distance_m;

        preview.edges = check_edges(
            &self.pattern,
            &frames.color,
            &self.geometry.color,
            &detection.homography,
        );
        preview.hint = analysis::choose_hint(&analysis::Observation {
            region_inside_images: analysis::region_inside_images(
                &self.geometry,
                &detection.homography,
            ),
            completeness: detection.quality.completeness(),
            spacing_px: detection.quality.spacing_px,
            depth,
            tilt,
            edges_block: self.options.require_visible_edges
                && matches!(preview.edges, EdgeCheck::Cropped { .. }),
        });
        Ok(preview)
    }

    /// Whether the last frame of a burst shows the pattern's whole border ring.
    fn burst_edges(&self, frames: &[Frames]) -> EdgeCheck {
        let Some(last) = frames.last() else {
            return EdgeCheck::Unknown;
        };
        let Ok(detection) = detect(&self.pattern, &last.color, &self.geometry.color) else {
            return EdgeCheck::Unknown;
        };
        check_edges(
            &self.pattern,
            &last.color,
            &self.geometry.color,
            &detection.homography,
        )
    }

    /// Captures a burst while the pattern is shown and measures the screen.
    pub fn measure_screen(
        &mut self,
        progress: &mut dyn FnMut(Step),
    ) -> Result<ScreenSummary, SessionError> {
        self.leave_marker_mode();
        let total = self.options.burst_frames.max(1);
        let frames = self.grab_burst(total, progress, |frames| frames)?;

        // The declared size is only a valid scale anchor if the screen shows
        // its whole image, so check the ring before spending time on the solve.
        let edges = self.burst_edges(&frames);
        if self.options.require_visible_edges
            && let EdgeCheck::Cropped { sides } = edges
        {
            self.failed_burst = Some(frames);
            return Err(SessionError::EdgesCropped(sides));
        }

        progress(Step::Solving);
        let alignment = self.gravity.camera_to_aligned();
        let options = SolveOptions {
            known_size: self.options.known_size,
            ..SolveOptions::default()
        };
        let fit = solve_screen(&self.geometry, &self.pattern, &frames, &alignment, &options)
            .inspect_err(|_| self.failed_burst = Some(frames.clone()))?;
        let summary = screen_summary(&fit, edges);
        let color = frames
            .last()
            .map(|f| f.color.clone())
            .unwrap_or_else(|| RgbImage::new(0, 0));
        self.failed_burst = None;
        self.screen = Some(ScreenMeasurement {
            fit,
            color,
            edges,
            gravity_aligned: self.gravity.is_ready(),
            frames,
        });
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
        self.marker_preview_ir = Some(frames.ir.clone());
        let mut preview = MarkerPreview {
            blobs: blobs.iter().map(|b| pt32(b.center)).collect(),
            ir: frames.ir,
            matched: None,
            hint: MarkerHint::Ready,
        };
        if blobs.is_empty() {
            preview.hint = MarkerHint::NoMarkers;
            return Ok(preview);
        }
        if blobs.len() < MARKER_COUNT {
            preview.hint = MarkerHint::TooFewMarkers { found: blobs.len() };
            return Ok(preview);
        }
        let Some(screen) = &self.screen else {
            return Ok(preview);
        };
        match markers::locate(&self.geometry, &screen.fit, &blobs) {
            Ok(located) => preview.matched = Some(located.blob_for_marker),
            Err(error) => {
                preview.hint = marker_hint(&error);
                report_marker_failure(
                    &mut self.marker_report_at,
                    &self.geometry,
                    &screen.fit,
                    &blobs,
                    &error,
                );
            }
        }
        Ok(preview)
    }

    /// Measures the six markers from a short burst of infrared frames. Needs
    /// [`measure_screen`](Self::measure_screen) first.
    pub fn measure_markers(
        &mut self,
        progress: &mut dyn FnMut(Step),
    ) -> Result<MarkerSummary, SessionError> {
        if self.screen.is_none() {
            return Err(SessionError::ScreenNotMeasured);
        }
        self.enter_marker_mode()?;
        let captured = self.grab_burst(MARKER_FRAMES, progress, |frames| {
            (markers::detect_blobs(&frames.ir), frames.ir)
        })?;
        let (bursts, ir_frames): (Vec<_>, Vec<_>) = captured.into_iter().unzip();
        // Kept even when the solve below fails: these are the frames it used.
        self.marker_burst_ir = ir_frames;
        progress(Step::Solving);
        let screen = self
            .screen
            .as_ref()
            .ok_or(SessionError::ScreenNotMeasured)?;
        let (summary, marker_px) = average_markers(&self.geometry, &screen.fit, &bursts)?;
        self.markers = Some(MarkerMeasurement {
            summary,
            marker_px,
            bursts,
        });
        progress(Step::Done);
        Ok(summary)
    }

    /// The screen and marker measurements, once both are made.
    fn measurements(&self) -> Result<(&ScreenMeasurement, &MarkerMeasurement), SessionError> {
        let screen = self
            .screen
            .as_ref()
            .ok_or(SessionError::ScreenNotMeasured)?;
        let markers = self
            .markers
            .as_ref()
            .ok_or(SessionError::MarkersNotMeasured)?;
        Ok((screen, markers))
    }

    /// The calibration and review data, once the screen and markers are measured.
    pub fn finish(&self) -> Result<Calibration, SessionError> {
        let (screen, markers) = self.measurements()?;
        self.assemble(screen, &screen.fit, markers.summary, markers.marker_px)
    }

    /// The same measurement as [`finish`](Self::finish) with the declared
    /// [`SessionOptions::known_size`] ignored: the size comes from the depth
    /// camera. It solves the stored frames again and needs no camera, so a
    /// review screen can offer it after the stream has stopped. Reuses the
    /// gravity frame captured with the screen, not later marker-phase IMU
    /// readings. Equals `finish` when no size was declared.
    pub fn finish_measured(&self) -> Result<Calibration, SessionError> {
        let (screen, markers) = self.measurements()?;
        if self.options.known_size.is_none() {
            return self.finish();
        }
        let alignment = screen.fit.camera_to_aligned;
        let fit = solve_screen(
            &self.geometry,
            &self.pattern,
            &screen.frames,
            &alignment,
            &SolveOptions::default(),
        )?;
        let (summary, marker_px) = average_markers(&self.geometry, &fit, &markers.bursts)?;
        self.assemble(screen, &fit, summary, marker_px)
    }

    fn assemble(
        &self,
        screen: &ScreenMeasurement,
        fit: &ScreenFit,
        markers: MarkerSummary,
        marker_px: [Point2<f32>; 6],
    ) -> Result<Calibration, SessionError> {
        let calibration = build_calibration(fit, &markers.points_m)?;
        let review = Review {
            color: screen.color.clone(),
            plane_rmse_m: fit.plane_rmse_m,
            plane_max_error_m: fit.plane_max_error_m,
            plane_inliers: fit.plane_inliers,
            screen_quad_px: fit.quad_px.map(pt32),
            marker_px,
            size_m: fit.size_m,
            diagonal_in: fit.diagonal_in(),
            size_source: fit.size_source,
            size_check: fit.size_check,
            edges: screen.edges,
            dots_used: fit.dots_used,
            dots_expected: self.pattern.dots().len(),
            seed_circle: markers.seed_circle,
            gravity_aligned: screen.gravity_aligned,
        };
        Ok(Calibration {
            calibration,
            review,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.leave_marker_mode();
    }
}
