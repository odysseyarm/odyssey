//! What one live frame says about whether the screen is ready to measure:
//! how bright the picture is, how much of the screen has depth, how far and
//! how obliquely it is seen, and the single hint that follows from it.

use image::RgbImage;
use nalgebra::{Matrix3, Point2, Vector3};

use super::Hint;
use crate::camera::{CameraGeometry, Frames};
use crate::imu::Tilt;
use crate::pattern::DetectFailure;
use crate::plane::{RansacParams, apply_homography, pinhole_ray, project_pinhole, ransac_plane};
use crate::solve::{self, SCREEN_INSET};

/// [`Hint::TooClose`] when the screen is nearer than this, meters.
const MIN_DISTANCE_M: f32 = 0.6;

/// [`Hint::TooFar`] when the screen is farther than this, meters.
const MAX_DISTANCE_M: f32 = 5.0;

/// Dots closer together than this many pixels are too small to localize well.
const MIN_SPACING_PX: f64 = 12.0;

/// [`Hint::CameraTiltedTooMuch`] beyond these tilts, degrees.
const MAX_PITCH_DEG: f64 = 35.0;
const MAX_ROLL_DEG: f64 = 20.0;

/// Cosine of the largest angle between the screen normal and the line of
/// sight.
const MIN_INCIDENCE_COS: f64 = 0.35;

/// Share of the pattern's dots that must be matched before the whole screen
/// counts as visible.
const MIN_PATTERN_COMPLETENESS: f64 = 0.8;

/// Share of the screen region that must have usable depth.
const MIN_DEPTH_COVERAGE: f32 = 0.6;

/// Largest root-mean-square distance from the plane fitted to the depth
/// samples of one frame, meters.
const MAX_PREVIEW_RMSE_M: f64 = 0.015;

/// Depth points the preview's plane fit works on. A plane on a few hundred
/// samples gives the viewing angle and a noise estimate without the full
/// burst.
const PREVIEW_PLANE_SAMPLES: usize = 1500;

/// Trials the preview's plane fit may spend.
const PREVIEW_PLANE_TRIALS: usize = 300;

/// Every n-th pixel of the picture is enough for a brightness estimate.
const LUMA_SAMPLE_STEP: usize = 7;

/// Luma at or above which a pixel counts as blown out.
const SATURATED_LUMA: usize = 250;

/// [`Hint::TooBright`] when more than this share of the sampled pixels is
/// blown out.
const MAX_SATURATED_SHARE: f64 = 0.15;

/// The brightness is judged by this percentile of the luma, so that a few
/// bright pixels do not hide a dark picture.
const BRIGHTNESS_PERCENTILE: f64 = 0.995;

/// [`Hint::TooDark`] when the [`BRIGHTNESS_PERCENTILE`] luma is below this.
const MIN_BRIGHTNESS_LUMA: usize = 60;

/// Blobs at which a failed grid search still suggests the screen is there but
/// partly out of view or hidden.
const MIN_BLOBS_FOR_PARTLY_VISIBLE: usize = 20;

/// Corners of the inset screen region in unit coordinates, in the order top
/// left, top right, bottom right, bottom left.
fn inset_corners() -> [(f64, f64); 4] {
    let (lo, hi) = SCREEN_INSET;
    [(lo, lo), (hi, lo), (hi, hi), (lo, hi)]
}

/// What a frame showed of a found pattern.
#[derive(Debug, Clone, Copy)]
pub(super) struct Observation {
    /// The inset screen region lies inside the color image; depth visibility
    /// is accounted for by `depth.coverage`.
    pub(super) region_inside_images: bool,
    /// Share of the pattern's dots that were matched.
    pub(super) completeness: f64,
    /// Typical pixel distance between neighboring dots.
    pub(super) spacing_px: f64,
    pub(super) depth: DepthStats,
    pub(super) tilt: Option<Tilt>,
    /// The display crops the pattern's border and the options refuse that.
    pub(super) edges_block: bool,
}

/// The most important thing to change, in order of what to fix first.
pub(super) fn choose_hint(observation: &Observation) -> Hint {
    let depth = &observation.depth;
    if !observation.region_inside_images || observation.completeness < MIN_PATTERN_COMPLETENESS {
        Hint::ScreenPartlyVisible
    } else if depth.distance_m.is_some_and(|d| d < MIN_DISTANCE_M) {
        Hint::TooClose
    } else if depth.distance_m.is_some_and(|d| d > MAX_DISTANCE_M)
        || observation.spacing_px < MIN_SPACING_PX
    {
        Hint::TooFar
    } else if observation
        .tilt
        .is_some_and(|t| t.pitch_deg.abs() > MAX_PITCH_DEG || t.roll_deg.abs() > MAX_ROLL_DEG)
    {
        Hint::CameraTiltedTooMuch
    } else if depth.cos_incidence.is_some_and(|c| c < MIN_INCIDENCE_COS) {
        Hint::ScreenTooOblique
    } else if depth.coverage.is_none_or(|c| c < MIN_DEPTH_COVERAGE)
        || depth
            .plane_rmse_m
            .is_none_or(|e| !e.is_finite() || e > MAX_PREVIEW_RMSE_M)
    {
        Hint::DepthPoor
    } else if observation.edges_block {
        Hint::EdgesCropped
    } else {
        Hint::Ready
    }
}

/// Whether the inset screen region lies inside the color image. The depth
/// imager's visibility is reflected separately in [`DepthStats::coverage`].
pub(super) fn region_inside_images(geometry: &CameraGeometry, unit_to_px: &Matrix3<f64>) -> bool {
    let color = &geometry.color;
    if color.width == 0 || color.height == 0 {
        return false;
    }
    inset_corners().into_iter().all(|(u, v)| {
        apply_homography(unit_to_px, Point2::new(u, v)).is_some_and(|p| {
            p.x >= 0.0
                && p.y >= 0.0
                && p.x <= f64::from(color.width - 1)
                && p.y <= f64::from(color.height - 1)
        })
    })
}

/// Sampled brightness of a picture.
struct LumaSummary {
    saturated: u32,
    count: u32,
    /// The [`BRIGHTNESS_PERCENTILE`] luma.
    percentile: usize,
}

fn summarize_luma(color: &RgbImage) -> LumaSummary {
    let mut histogram = [0u32; 256];
    let mut saturated = 0u32;
    let mut count = 0u32;
    for p in color.pixels().step_by(LUMA_SAMPLE_STEP) {
        // Integer Rec. 601 luma: weights 0.299, 0.587, 0.114 in 1/256ths.
        let luma =
            ((77 * u32::from(p[0]) + 150 * u32::from(p[1]) + 29 * u32::from(p[2])) >> 8) as usize;
        histogram[luma] += 1;
        count += 1;
        if luma >= SATURATED_LUMA {
            saturated += 1;
        }
    }
    let target = (BRIGHTNESS_PERCENTILE * f64::from(count)) as u32;
    let mut accumulated = 0;
    let percentile = histogram
        .iter()
        .position(|&n| {
            accumulated += n;
            accumulated >= target
        })
        .unwrap_or(255);
    LumaSummary {
        saturated,
        count,
        percentile,
    }
}

/// The hint for a frame in which the pattern was not found: bad exposure
/// first, then a partly visible screen, else just not found.
pub(super) fn hint_without_pattern(color: &RgbImage, failure: DetectFailure) -> Hint {
    let luma = summarize_luma(color);
    if f64::from(luma.saturated) > MAX_SATURATED_SHARE * f64::from(luma.count) {
        Hint::TooBright
    } else if luma.percentile < MIN_BRIGHTNESS_LUMA {
        Hint::TooDark
    } else if matches!(failure, DetectFailure::NoGrid { blobs } if blobs >= MIN_BLOBS_FOR_PARTLY_VISIBLE)
    {
        Hint::ScreenPartlyVisible
    } else {
        Hint::ScreenNotFound
    }
}

/// Distance, depth coverage and plane quality of the screen region in one
/// frame; `None` where the frame has no depth to say.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DepthStats {
    pub(super) coverage: Option<f32>,
    pub(super) distance_m: Option<f32>,
    pub(super) cos_incidence: Option<f64>,
    pub(super) plane_rmse_m: Option<f64>,
}

/// Area of a quadrilateral by the shoelace formula.
fn quad_area(c: &[Point2<f64>; 4]) -> f64 {
    (0..4)
        .map(|i| c[i].x * c[(i + 1) % 4].y - c[(i + 1) % 4].x * c[i].y)
        .sum::<f64>()
        .abs()
        / 2.0
}

/// The inset screen region in depth pixels, assuming the screen is flat at
/// `distance` meters in front of the color camera. `None` when a corner
/// cannot be projected.
fn inset_region_in_depth_px(
    geometry: &CameraGeometry,
    unit_to_px: &Matrix3<f64>,
    distance: f64,
) -> Option<[Point2<f64>; 4]> {
    let mut corners = [Point2::origin(); 4];
    for (slot, (u, v)) in corners.iter_mut().zip(inset_corners()) {
        let px = apply_homography(unit_to_px, Point2::new(u, v))?;
        let ray = pinhole_ray(&geometry.color, px);
        *slot = project_pinhole(
            &geometry.depth,
            &geometry.color_to_depth.apply(&(ray * distance)),
        )?;
    }
    Some(corners)
}

pub(super) fn depth_stats(
    geometry: &CameraGeometry,
    frames: &Frames,
    unit_to_px: &Matrix3<f64>,
) -> DepthStats {
    let color_from_depth = geometry.color_to_depth.inverse();
    let points = solve::screen_depth_points(
        geometry,
        std::slice::from_ref(frames),
        unit_to_px,
        &color_from_depth,
    );
    if points.is_empty() {
        return DepthStats {
            coverage: Some(0.0),
            ..DepthStats::default()
        };
    }

    let mut z: Vec<f64> = points.iter().map(|p| p.z).collect();
    z.sort_by(f64::total_cmp);
    let distance = z[z.len() / 2];

    // How many samples a fully valid region would yield at this distance.
    let stride = f64::from(solve::depth_stride(frames.depth.width()));
    let coverage = inset_region_in_depth_px(geometry, unit_to_px, distance).map(|corners| {
        let area = quad_area(&corners);
        ((points.len() as f64 * stride * stride) / area.max(1.0)).min(1.0) as f32
    });

    let step = (points.len() / PREVIEW_PLANE_SAMPLES).max(1);
    let subset: Vec<Vector3<f64>> = points.iter().step_by(step).copied().collect();
    let fit = ransac_plane(
        &subset,
        &RansacParams {
            max_trials: PREVIEW_PLANE_TRIALS,
            score_points: PREVIEW_PLANE_SAMPLES,
            ..RansacParams::default()
        },
    );
    DepthStats {
        coverage,
        distance_m: Some(distance as f32),
        cos_incidence: fit
            .as_ref()
            .map(|f| f.plane.normal.dot(&f.plane.point.normalize())),
        plane_rmse_m: fit.as_ref().map(|f| f.rmse),
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// A view that passes every check.
    fn ready() -> Observation {
        Observation {
            region_inside_images: true,
            completeness: 1.0,
            spacing_px: 30.0,
            depth: DepthStats {
                coverage: Some(0.9),
                distance_m: Some(2.0),
                cos_incidence: Some(0.9),
                plane_rmse_m: Some(0.003),
            },
            tilt: Some(Tilt {
                pitch_deg: 2.0,
                roll_deg: -1.0,
            }),
            edges_block: false,
        }
    }

    #[test]
    fn a_clean_view_is_ready() {
        assert_eq!(choose_hint(&ready()), Hint::Ready);
    }

    /// Spoils one aspect of a ready observation.
    type Spoil = fn(&mut Observation);

    #[test]
    fn each_problem_maps_to_its_hint() {
        let cases: [(Spoil, Hint); 12] = [
            (
                |o| o.region_inside_images = false,
                Hint::ScreenPartlyVisible,
            ),
            (|o| o.completeness = 0.5, Hint::ScreenPartlyVisible),
            (|o| o.depth.distance_m = Some(0.4), Hint::TooClose),
            (|o| o.depth.distance_m = Some(6.0), Hint::TooFar),
            (|o| o.spacing_px = 8.0, Hint::TooFar),
            (
                |o| {
                    o.tilt = Some(Tilt {
                        pitch_deg: 40.0,
                        roll_deg: 0.0,
                    });
                },
                Hint::CameraTiltedTooMuch,
            ),
            (
                |o| o.depth.cos_incidence = Some(0.2),
                Hint::ScreenTooOblique,
            ),
            (|o| o.depth.coverage = None, Hint::DepthPoor),
            (|o| o.depth.plane_rmse_m = Some(0.05), Hint::DepthPoor),
            (|o| o.depth.plane_rmse_m = None, Hint::DepthPoor),
            (|o| o.depth.plane_rmse_m = Some(f64::NAN), Hint::DepthPoor),
            (|o| o.edges_block = true, Hint::EdgesCropped),
        ];
        for (break_it, expected) in cases {
            let mut observation = ready();
            break_it(&mut observation);
            assert_eq!(choose_hint(&observation), expected);
        }
    }

    #[test]
    fn earlier_problems_take_precedence() {
        let mut observation = ready();
        observation.edges_block = true;
        observation.depth.distance_m = Some(0.4);
        observation.completeness = 0.5;
        assert_eq!(choose_hint(&observation), Hint::ScreenPartlyVisible);
        observation.completeness = 1.0;
        assert_eq!(choose_hint(&observation), Hint::TooClose);
    }

    #[test]
    fn missing_pattern_is_blamed_on_exposure_before_visibility() {
        let flat = |level: u8| RgbImage::from_pixel(64, 64, Rgb([level; 3]));
        let grid = DetectFailure::NoGrid { blobs: 25 };
        assert_eq!(hint_without_pattern(&flat(255), grid), Hint::TooBright);
        assert_eq!(hint_without_pattern(&flat(10), grid), Hint::TooDark);
        assert_eq!(
            hint_without_pattern(&flat(128), grid),
            Hint::ScreenPartlyVisible
        );
        assert_eq!(
            hint_without_pattern(&flat(128), DetectFailure::NoGrid { blobs: 5 }),
            Hint::ScreenNotFound
        );
        assert_eq!(
            hint_without_pattern(&flat(128), DetectFailure::NoDots { blobs: 0 }),
            Hint::ScreenNotFound
        );
    }
}
