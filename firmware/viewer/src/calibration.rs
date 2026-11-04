//! Native-pixel calibration shared by the integrated tool and file utilities.
//! Mono intrinsics are fitted in image pixels; only exported camera matrices
//! are scaled into the selected sensor's tracking coordinate system. Stereo
//! translation is in metres, from wide-field camera to near-field camera.
use std::sync::atomic::{AtomicBool, Ordering};

use nalgebra::{Dyn, OMatrix, U2, Vector3};
use opencv::calib3d::{
    CALIB_FIX_INTRINSIC, calibrate_camera, project_points_def, stereo_calibrate,
};
use opencv::core::{
    Mat, MatTraitConst, MatTraitConstManual, Point2f, Point3f, TermCriteria, TermCriteria_COUNT,
    TermCriteria_EPS, Vector, no_array,
};
use serde_json::{Value, json};

use crate::aprilgrid::{AprilGrid, AprilGridConfig};
use crate::{Port, ViewerError, chessboard, circles};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    AprilGrid,
    Chessboard,
    SymmetricCircles,
    AsymmetricCircles,
}
impl Pattern {
    pub fn label(self) -> &'static str {
        match self {
            Self::AprilGrid => "AprilGrid",
            Self::Chessboard => "Chessboard",
            Self::SymmetricCircles => "Symmetric circles",
            Self::AsymmetricCircles => "Asymmetric circles",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub pattern: Pattern,
    pub rows: u16,
    pub cols: u16,
    /// Square/centre spacing, or AprilGrid tag pitch, in millimetres.
    pub pitch_mm: f64,
    /// Circle diameter or AprilGrid tag side, in millimetres.
    pub feature_mm: f64,
}
impl Default for Target {
    fn default() -> Self {
        Self {
            pattern: Pattern::Chessboard,
            rows: 6,
            cols: 8,
            pitch_mm: 25.0,
            feature_mm: 20.0,
        }
    }
}
impl Target {
    pub fn validate(&self) -> Result<(), ViewerError> {
        if self.rows < 2
            || self.cols < 2
            || u32::from(self.rows) * u32::from(self.cols) > 4096
            || !self.pitch_mm.is_finite()
            || self.pitch_mm <= 0.0
            || !self.feature_mm.is_finite()
            || self.feature_mm <= 0.0
        {
            return Err(ViewerError::InvalidCalibration(
                "positive target dimensions and pitch are required",
            ));
        }
        if self.pattern != Pattern::Chessboard && self.feature_mm >= self.pitch_mm {
            return Err(ViewerError::InvalidCalibration(
                "feature diameter/tag side must be smaller than pitch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Observation {
    pub image: Vec<[f32; 2]>,
    pub object: Vec<[f32; 3]>,
}

/// Detect at native resolution, reusing the original viewer detectors. No GUI,
/// device access, or file writes occur here.
pub fn detect(
    pixels: &[u8],
    resolution: (u16, u16),
    target: &Target,
) -> Result<Option<Observation>, ViewerError> {
    target.validate()?;
    if resolution.0 < 2
        || resolution.1 < 2
        || pixels.len() != usize::from(resolution.0) * usize::from(resolution.1)
    {
        return Err(ViewerError::InvalidImageBuffer);
    }
    let native = (u32::from(resolution.0), u32::from(resolution.1));
    let pitch = (target.pitch_mm / 1000.0) as f32;
    let (image, object) = match target.pattern {
        Pattern::AprilGrid => {
            let grid = AprilGrid::new(AprilGridConfig::new(
                target.rows,
                target.cols,
                target.feature_mm / 1000.0,
                (target.pitch_mm - target.feature_mm) / 1000.0,
            ));
            let Some(points) = grid.get_corner_and_object_points(pixels, resolution, native)?
            else {
                return Ok(None);
            };
            points
        }
        Pattern::Chessboard => {
            let Some(image) = chessboard::get_chessboard_corners(
                pixels,
                Port::Nf,
                resolution,
                native,
                target.rows,
                target.cols,
                false,
            )?
            else {
                return Ok(None);
            };
            if image.len() != usize::from(target.rows) * usize::from(target.cols) {
                return Ok(None);
            }
            let mut object = Vector::new();
            for row in 0..target.rows {
                for col in 0..target.cols {
                    object.push(Point3f::new(
                        f32::from(col) * pitch,
                        f32::from(row) * pitch,
                        0.0,
                    ));
                }
            }
            (image, object)
        }
        Pattern::SymmetricCircles | Pattern::AsymmetricCircles => {
            let asymmetric = target.pattern == Pattern::AsymmetricCircles;
            let found = circles::get_circles_centers(
                pixels,
                Port::Nf,
                resolution,
                native,
                target.rows,
                target.cols,
                asymmetric,
                false,
            )?;
            if !found.pattern_found {
                return Ok(None);
            }
            (
                found.points,
                circles::board_points(target.rows, target.cols, pitch, asymmetric),
            )
        }
    };
    if image.len() < 6 || image.len() != object.len() {
        return Ok(None);
    }
    Ok(Some(Observation {
        image: image.iter().map(|p| [p.x, p.y]).collect(),
        object: object.iter().map(|p| [p.x, p.y, p.z]).collect(),
    }))
}

#[derive(Debug, Clone)]
pub struct MonoCalibration {
    /// Row-major camera matrix in native image pixels.
    pub matrix: [f64; 9],
    pub distortion: [f64; 5],
    pub rms: f64,
    pub view_errors: Vec<f64>,
    pub resolution: (u16, u16),
}
impl MonoCalibration {
    /// Compatible with odyssey-common's calibration JSON importer and OpenCV.
    pub fn json(&self, scale: (f64, f64), target: &Target) -> Result<String, ViewerError> {
        let mut matrix = self.matrix;
        for x in &mut matrix[..3] {
            *x *= scale.0;
        }
        for y in &mut matrix[3..6] {
            *y *= scale.1;
        }
        Ok(serde_json::to_string_pretty(&json!({
            "camera_matrix": matrix_json(3, 3, &matrix), "dist_coeffs": matrix_json(1, 5, &self.distortion),
            "rms_error": self.rms, "error_units": "native image pixels", "per_view_errors": self.view_errors,
            "num_captures": self.view_errors.len(), "image_width": self.resolution.0, "image_height": self.resolution.1,
            "method": target.pattern.label(), "pitch_mm": target.pitch_mm, "version": 1
        }))?)
    }
}

#[derive(Debug, Clone)]
pub struct StereoCalibration {
    pub rotation: [f64; 9],
    pub translation: [f64; 3],
    pub rms: f64,
    pub captures: usize,
}
impl StereoCalibration {
    pub fn json(&self, target: &Target) -> Result<String, ViewerError> {
        Ok(serde_json::to_string_pretty(
            &json!({"r": matrix_json(3,3,&self.rotation), "t": matrix_json(3,1,&self.translation), "rms_error": self.rms, "error_units": "native image pixels", "translation_units": "metres", "num_captures": self.captures, "method": target.pattern.label(), "version": 1}),
        )?)
    }
}

fn matrix_json(rows: usize, cols: usize, data: &[f64]) -> Value {
    json!({"type_id":"opencv-matrix", "rows":rows, "cols":cols, "dt":"d", "data":data})
}
fn criteria() -> TermCriteria {
    TermCriteria {
        typ: TermCriteria_COUNT + TermCriteria_EPS,
        max_count: 100,
        epsilon: 1e-8,
    }
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), ViewerError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ViewerError::Cancelled)
    } else {
        Ok(())
    }
}
fn object_vector(observation: &Observation) -> Vector<Point3f> {
    observation
        .object
        .iter()
        .map(|p| Point3f::new(p[0], p[1], p[2]))
        .collect()
}
fn image_vector(observation: &Observation) -> Vector<Point2f> {
    observation
        .image
        .iter()
        .map(|p| Point2f::new(p[0], p[1]))
        .collect()
}
fn mat3(data: &[f64; 9]) -> Result<Mat, ViewerError> {
    Ok(Mat::from_slice_2d(&[
        &data[0..3],
        &data[3..6],
        &data[6..9],
    ])?)
}
fn values<const N: usize>(mat: &Mat) -> Result<[f64; N], ViewerError> {
    mat.data_typed::<f64>()?
        .try_into()
        .map_err(|_| ViewerError::InvalidCalibration("unexpected solver matrix dimensions"))
}
fn validate_observation(view: &Observation) -> Result<(), ViewerError> {
    if view.image.len() < 6
        || view.image.len() != view.object.len()
        || view.image.iter().flatten().any(|x| !x.is_finite())
        || view.object.iter().flatten().any(|x| !x.is_finite())
    {
        return Err(ViewerError::InvalidCalibration(
            "incomplete or nonfinite target observations",
        ));
    }
    Ok(())
}
fn validate_views(views: &[Observation]) -> Result<(), ViewerError> {
    if views.len() < 3 {
        return Err(ViewerError::InvalidCalibration(
            "capture at least three different target poses",
        ));
    }
    for view in views {
        validate_observation(view)?;
    }
    Ok(())
}

/// Reuses the original OpenCV chessboard/AprilGrid fitting and the ccalib
/// perspective-correct circle-centre solver. Cancellation is checked around
/// each bounded solver call; callers retain and join the worker on cancellation.
pub fn solve_mono(
    views: &[Observation],
    resolution: (u16, u16),
    target: &Target,
    port: Port,
    cancel: &AtomicBool,
) -> Result<MonoCalibration, ViewerError> {
    target.validate()?;
    if resolution.0 < 2 || resolution.1 < 2 {
        return Err(ViewerError::InvalidImageBuffer);
    }
    validate_views(views)?;
    check_cancel(cancel)?;
    if matches!(
        target.pattern,
        Pattern::SymmetricCircles | Pattern::AsymmetricCircles
    ) {
        let object: Vec<_> = views[0]
            .object
            .iter()
            .map(|p| Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])))
            .collect();
        if views.iter().any(|v| v.object != views[0].object) {
            return Err(ViewerError::InvalidCalibration(
                "circle observations must share the same complete target",
            ));
        }
        let images: Vec<_> = views
            .iter()
            .map(|v| {
                OMatrix::<f64, U2, Dyn>::from_iterator(
                    v.image.len(),
                    v.image
                        .iter()
                        .flat_map(|p| [f64::from(p[0]), f64::from(p[1])]),
                )
            })
            .collect();
        let radius = target.feature_mm / 2000.0;
        let mut flags = ccalib::Flags::empty();
        flags.set(ccalib::Flags::RADIAL_DISTORTION, port == Port::Wf);
        let result = ccalib::calibrate(
            &object,
            &images,
            radius,
            f64::from(resolution.0) * if port == Port::Nf { 1.5 } else { 0.35 },
            f64::from(resolution.0 - 1) / 2.0,
            f64::from(resolution.1 - 1) / 2.0,
            Vector3::new(0.0, 0.0, target.pitch_mm / 1000.0 * 10.0),
            flags,
        )?;
        check_cancel(cancel)?;
        let k = result.intrinsics;
        let view_errors = images
            .iter()
            .zip(&result.extrinsics)
            .map(|(image, pose)| {
                (object
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        (ccalib::reproject(&k, pose.r, pose.t, *p, radius) - image.column(i))
                            .norm_squared()
                    })
                    .sum::<f64>()
                    / object.len() as f64)
                    .sqrt()
            })
            .collect();
        return Ok(MonoCalibration {
            matrix: [k.fx, 0.0, k.cx, 0.0, k.fy, k.cy, 0.0, 0.0, 1.0],
            distortion: k.distortion.unwrap_or([0.0; 5]),
            rms: result.rmse,
            view_errors,
            resolution,
        });
    }
    let objects: Vector<Vector<Point3f>> = views.iter().map(object_vector).collect();
    let images: Vector<Vector<Point2f>> = views.iter().map(image_vector).collect();
    let mut matrix = Mat::default();
    let mut distortion = Mat::default();
    let mut rotations = Vector::<Mat>::new();
    let mut translations = Vector::<Mat>::new();
    let rms = calibrate_camera(
        &objects,
        &images,
        (i32::from(resolution.0), i32::from(resolution.1)).into(),
        &mut matrix,
        &mut distortion,
        &mut rotations,
        &mut translations,
        0,
        criteria(),
    )?;
    check_cancel(cancel)?;
    let mut view_errors = Vec::with_capacity(views.len());
    for (i, view) in views.iter().enumerate() {
        check_cancel(cancel)?;
        let mut projected = Vector::<Point2f>::new();
        project_points_def(
            &objects.get(i)?,
            &rotations.get(i)?,
            &translations.get(i)?,
            &matrix,
            &distortion,
            &mut projected,
        )?;
        view_errors.push(
            (projected
                .iter()
                .zip(&view.image)
                .map(|(p, q)| f64::from((p.x - q[0]).powi(2) + (p.y - q[1]).powi(2)))
                .sum::<f64>()
                / view.image.len() as f64)
                .sqrt(),
        );
    }
    if !rms.is_finite() {
        return Err(ViewerError::InvalidCalibration(
            "nonfinite calibration result",
        ));
    }
    Ok(MonoCalibration {
        matrix: values(&matrix)?,
        distortion: values(&distortion)?,
        rms,
        view_errors,
        resolution,
    })
}

/// Each pair is explicitly captured with the board and device stationary while
/// switching cameras. AprilGrid pairs intersect tag-corner identities, never zip
/// unrelated partial detections. Both monocular intrinsics remain fixed.
pub fn solve_stereo(
    pairs: &[(Observation, Observation)],
    wf: &MonoCalibration,
    nf: &MonoCalibration,
    cancel: &AtomicBool,
) -> Result<StereoCalibration, ViewerError> {
    if wf.resolution != nf.resolution {
        return Err(ViewerError::InvalidCalibration(
            "stereo image dimensions differ",
        ));
    }
    let mut objects = Vector::<Vector<Point3f>>::new();
    let mut wide = Vector::<Vector<Point2f>>::new();
    let mut near = Vector::<Vector<Point2f>>::new();
    for (w, n) in pairs {
        check_cancel(cancel)?;
        validate_observation(w)?;
        validate_observation(n)?;
        let mut obj = Vector::new();
        let mut wp = Vector::new();
        let mut np = Vector::new();
        for (i, p) in w.object.iter().enumerate() {
            if let Some(j) = n.object.iter().position(|q| p == q) {
                obj.push(Point3f::new(p[0], p[1], p[2]));
                wp.push(Point2f::new(w.image[i][0], w.image[i][1]));
                np.push(Point2f::new(n.image[j][0], n.image[j][1]));
            }
        }
        if obj.len() >= 6 {
            objects.push(obj);
            wide.push(wp);
            near.push(np);
        }
    }
    if objects.len() < 3 {
        return Err(ViewerError::InvalidCalibration(
            "at least three paired poses with six common target points are required",
        ));
    }
    let mut wk = mat3(&wf.matrix)?;
    let mut nk = mat3(&nf.matrix)?;
    let mut wd = Mat::from_slice(&wf.distortion)?.try_clone()?;
    let mut nd = Mat::from_slice(&nf.distortion)?.try_clone()?;
    let mut rotation = Mat::default();
    let mut translation = Mat::default();
    let rms = stereo_calibrate(
        &objects,
        &wide,
        &near,
        &mut wk,
        &mut wd,
        &mut nk,
        &mut nd,
        (i32::from(wf.resolution.0), i32::from(wf.resolution.1)).into(),
        &mut rotation,
        &mut translation,
        &mut no_array(),
        &mut no_array(),
        CALIB_FIX_INTRINSIC,
        criteria(),
    )?;
    check_cancel(cancel)?;
    if !rms.is_finite() {
        return Err(ViewerError::InvalidCalibration(
            "nonfinite stereo calibration result",
        ));
    }
    Ok(StereoCalibration {
        rotation: values(&rotation)?,
        translation: values(&translation)?,
        rms,
        captures: objects.len(),
    })
}

/// Read an image file for offline calibration without a second capture UI.
pub fn load_image(path: &std::path::Path) -> Result<(u16, u16, Vec<u8>), ViewerError> {
    let image = image::ImageReader::open(path)?.decode()?.into_luma8();
    let width = u16::try_from(image.width()).map_err(|_| ViewerError::InvalidImageBuffer)?;
    let height = u16::try_from(image.height()).map_err(|_| ViewerError::InvalidImageBuffer)?;
    if width < 2 || height < 2 {
        return Err(ViewerError::InvalidImageBuffer);
    }
    Ok((width, height, image.into_raw()))
}

/// Preserve captured grayscale images for reproducible calibration datasets.
pub fn save_image(
    path: &std::path::Path,
    width: u16,
    height: u16,
    pixels: &[u8],
) -> Result<(), ViewerError> {
    if pixels.len() != usize::from(width) * usize::from(height) {
        return Err(ViewerError::InvalidImageBuffer);
    }
    image::save_buffer_with_format(
        path,
        pixels,
        u32::from(width),
        u32::from(height),
        image::ColorType::L8,
        image::ImageFormat::Png,
    )?;
    Ok(())
}
