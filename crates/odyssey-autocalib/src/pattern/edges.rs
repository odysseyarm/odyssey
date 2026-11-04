//! Detecting whether the display shows the pattern's border ring, i.e. whether it crops the
//! image (overscan).

use image::RgbImage;
use nalgebra::{Matrix3, Point2};

use super::{FRAME_THICKNESS_H, LUMA_WEIGHTS, Pattern};
use crate::camera::Intrinsics;
use crate::plane::{apply_homography, distort_pixel};

/// A set of screen sides. The ring on a side is the pattern's
/// outermost pixel row or column there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SideSet {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl SideSet {
    pub fn is_empty(&self) -> bool {
        !(self.top || self.right || self.bottom || self.left)
    }

    pub fn count(&self) -> usize {
        [self.top, self.right, self.bottom, self.left]
            .into_iter()
            .filter(|&s| s)
            .count()
    }

    fn insert(&mut self, side: Side) {
        match side {
            Side::Top => self.top = true,
            Side::Right => self.right = true,
            Side::Bottom => self.bottom = true,
            Side::Left => self.left = true,
        }
    }
}

/// Whether the pattern's border ring reaches the camera, i.e. whether the
/// screen shows its whole image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeCheck {
    /// The ring is visible on all four sides.
    Visible,
    /// The ring is missing or broken on these sides: the display crops the
    /// image (overscan) or something covers its border.
    Cropped { sides: SideSet },
    /// The ring cannot be judged: too faint or small in the picture, or partly
    /// outside the camera's view. Never a reason to refuse a measurement.
    Unknown,
}

const EDGE_SAMPLES_PER_SIDE: usize = 24;
/// Samples run over this share of a side, which keeps them clear of the corner brackets.
const EDGE_SPAN: (f64, f64) = (0.15, 0.85);
const EDGE_MIN_VALID_SAMPLES: usize = 6;
/// A side is visible when this share of its samples shows a ridge.
const EDGE_VISIBLE_FRACTION: f64 = 0.7;
/// Dots must stand out from the black by this many levels for the ring to be judged.
const EDGE_MIN_CONTRAST: f64 = 40.0;
/// A ridge must rise above both neighbors by this share of the dot contrast.
const EDGE_RIDGE_FRACTION: f64 = 0.25;
/// Rings narrower than this many pixels peak well below full brightness, so a
/// bright wall behind a cropped ring would look like a ridge. Such samples are
/// not judged.
const EDGE_MIN_RING_PX: f64 = 1.2;
/// How far inward from the ring, in screen heights, a sample probes to find the ring's
/// direction and apparent width in the image.
const RING_PROBE_H: f64 = 0.01;
/// A sample compares the ring with points this many ring widths to either side ...
const RIDGE_OFFSET_RING_WIDTHS: f64 = 2.0;
/// ... but never closer than this many pixels, which would sit inside the blur of the ring ...
const RIDGE_MIN_OFFSET_PX: f64 = 2.5;
/// ... nor farther than this many pixels, which would leave the border's neighborhood.
const RIDGE_MAX_OFFSET_PX: f64 = 8.0;

#[derive(Debug, Clone, Copy)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    const ALL: [Self; 4] = [Self::Top, Self::Right, Self::Bottom, Self::Left];

    /// Direction from the ring toward the screen's middle.
    fn inward(self) -> (f64, f64) {
        match self {
            Self::Top => (0.0, 1.0),
            Self::Right => (-1.0, 0.0),
            Self::Bottom => (0.0, -1.0),
            Self::Left => (1.0, 0.0),
        }
    }

    /// The point `t` of the way along the ring's center line on this side, in the pattern's
    /// units (see [`Pattern::frame_rects`]).
    fn ring_point(self, aspect: f64, t: f64, half_thickness: f64) -> (f64, f64) {
        match self {
            Self::Top => (aspect * t, half_thickness),
            Self::Right => (aspect - half_thickness, t),
            Self::Bottom => (aspect * t, 1.0 - half_thickness),
            Self::Left => (half_thickness, t),
        }
    }
}

fn luma_at(image: &RgbImage, x: u32, y: u32) -> f64 {
    let p = image.get_pixel(x, y);
    let [r, g, b] = LUMA_WEIGHTS.map(f64::from);
    (r * f64::from(p[0]) + g * f64::from(p[1]) + b * f64::from(p[2])) / 256.0
}

/// Brightest pixel in the 3x3 block around `p`; absorbs a pixel of
/// registration error. `None` unless the block lies inside the image.
fn luma_peak(image: &RgbImage, p: Point2<f64>) -> Option<f64> {
    let (x, y) = (p.x.round(), p.y.round());
    if !x.is_finite()
        || !y.is_finite()
        || x < 1.0
        || y < 1.0
        || x + 2.0 > f64::from(image.width())
        || y + 2.0 > f64::from(image.height())
    {
        return None;
    }
    let (x, y) = (x as u32, y as u32);
    (y - 1..=y + 1)
        .flat_map(|yy| (x - 1..=x + 1).map(move |xx| (xx, yy)))
        .map(|(xx, yy)| luma_at(image, xx, yy))
        .reduce(f64::max)
}

/// Brightness at the subpixel position `p`. `None` unless the interpolation block lies inside
/// the image.
fn luma_bilinear(image: &RgbImage, p: Point2<f64>) -> Option<f64> {
    let (x0, y0) = (p.x.floor(), p.y.floor());
    if !x0.is_finite()
        || !y0.is_finite()
        || x0 < 0.0
        || y0 < 0.0
        || x0 + 2.0 > f64::from(image.width())
        || y0 + 2.0 > f64::from(image.height())
    {
        return None;
    }
    let (fx, fy) = (p.x - x0, p.y - y0);
    let (x0, y0) = (x0 as u32, y0 as u32);
    let top = luma_at(image, x0, y0) * (1.0 - fx) + luma_at(image, x0 + 1, y0) * fx;
    let bottom = luma_at(image, x0, y0 + 1) * (1.0 - fx) + luma_at(image, x0 + 1, y0 + 1) * fx;
    Some(top * (1.0 - fy) + bottom * fy)
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

/// The image and the pose of the pattern in it, with the conversions between the pattern's
/// physical units (`x` in `0..aspect`, `y` in `0..1`, screen heights) and raw pixels.
struct RingView<'a> {
    pattern: &'a Pattern,
    image: &'a RgbImage,
    intrinsics: &'a Intrinsics,
    unit_to_pixel: &'a Matrix3<f64>,
}

impl RingView<'_> {
    /// Undistorted pixel at the physical pattern position `(x, y)`.
    fn undistorted(&self, x: f64, y: f64) -> Option<Point2<f64>> {
        apply_homography(self.unit_to_pixel, Point2::new(x / self.pattern.aspect, y))
    }

    fn raw(&self, p: Point2<f64>) -> Point2<f64> {
        distort_pixel(self.intrinsics, p)
    }

    /// How much brighter the dots are than the black between them, or `None` if the pattern
    /// is not in view well enough to measure it.
    fn dot_contrast(&self) -> Option<f64> {
        let pattern = self.pattern;
        let aspect = pattern.aspect;
        let mut dot_levels: Vec<f64> = pattern
            .dots
            .iter()
            .filter_map(|d| self.undistorted(d.u * aspect, d.v))
            .filter_map(|p| luma_peak(self.image, self.raw(p)))
            .collect();
        let mut black_levels = Vec::new();
        for r in 0..pattern.rows - 1 {
            for c in 0..pattern.cols - 1 {
                let (a, b) = (pattern.dot_at(c, r), pattern.dot_at(c + 1, r + 1));
                let center = self.undistorted((a.u + b.u) / 2.0 * aspect, (a.v + b.v) / 2.0);
                if let Some(v) = center.and_then(|p| luma_bilinear(self.image, self.raw(p))) {
                    black_levels.push(v);
                }
            }
        }
        Some(median(&mut dot_levels)? - median(&mut black_levels)?)
    }

    /// Whether the ring on `side` shows a ridge at enough of its sample points.
    fn judge_side(&self, side: Side, margin: f64) -> SideVerdict {
        let aspect = self.pattern.aspect;
        let half = FRAME_THICKNESS_H / 2.0;
        let inward = side.inward();
        let (mut valid, mut ridges) = (0usize, 0usize);
        for k in 0..EDGE_SAMPLES_PER_SIDE {
            let t = EDGE_SPAN.0
                + (EDGE_SPAN.1 - EDGE_SPAN.0) * k as f64 / (EDGE_SAMPLES_PER_SIDE - 1) as f64;
            let (x, y) = side.ring_point(aspect, t, half);
            let (Some(on_ring), Some(toward)) = (
                self.undistorted(x, y),
                self.undistorted(x + inward.0 * RING_PROBE_H, y + inward.1 * RING_PROBE_H),
            ) else {
                continue;
            };
            // How wide the ring appears here: perspective squeezes it on the
            // far side and at steep angles, where it fades below what the
            // camera resolves. Such samples say nothing either way.
            let step = toward - on_ring;
            let ring_px = step.norm() * FRAME_THICKNESS_H / RING_PROBE_H;
            if ring_px < EDGE_MIN_RING_PX {
                continue;
            }
            let direction = step.normalize();
            let offset = (RIDGE_OFFSET_RING_WIDTHS * ring_px)
                .clamp(RIDGE_MIN_OFFSET_PX, RIDGE_MAX_OFFSET_PX);
            let sample = (
                luma_peak(self.image, self.raw(on_ring)),
                luma_bilinear(self.image, self.raw(on_ring + direction * offset)),
                luma_bilinear(self.image, self.raw(on_ring - direction * offset)),
            );
            if let (Some(ring), Some(inner), Some(outer)) = sample {
                valid += 1;
                if ring - inner.max(outer) > margin {
                    ridges += 1;
                }
            }
        }
        if valid < EDGE_MIN_VALID_SAMPLES {
            SideVerdict::Unknown
        } else if (ridges as f64) < EDGE_VISIBLE_FRACTION * valid as f64 {
            SideVerdict::Cropped
        } else {
            SideVerdict::Visible
        }
    }
}

enum SideVerdict {
    Visible,
    Cropped,
    Unknown,
}

/// Looks for the pattern's border ring in a color image, given the
/// homography from pattern unit coordinates to undistorted pixels that
/// [`detect`](super::detect) found.
///
/// The ring is thin, so each sample compares the brightness on the ring's
/// predicted center line with the brightness a few pixels to either side and
/// asks for a ridge. A ridge needs no assumption about what lies outside the
/// screen: a cropped ring leaves the bezel or wall there, which is flat, and
/// the test fails however bright the room is.
pub fn check_edges(
    pattern: &Pattern,
    image: &RgbImage,
    intrinsics: &Intrinsics,
    unit_to_pixel: &Matrix3<f64>,
) -> EdgeCheck {
    let view = RingView {
        pattern,
        image,
        intrinsics,
        unit_to_pixel,
    };
    let Some(contrast) = view.dot_contrast() else {
        return EdgeCheck::Unknown;
    };
    if contrast < EDGE_MIN_CONTRAST {
        return EdgeCheck::Unknown;
    }

    let margin = EDGE_RIDGE_FRACTION * contrast;
    let mut cropped = SideSet::default();
    let mut unknown = false;
    for side in Side::ALL {
        match view.judge_side(side, margin) {
            SideVerdict::Visible => {}
            SideVerdict::Cropped => cropped.insert(side),
            SideVerdict::Unknown => unknown = true,
        }
    }
    if !cropped.is_empty() {
        EdgeCheck::Cropped { sides: cropped }
    } else if unknown {
        EdgeCheck::Unknown
    } else {
        EdgeCheck::Visible
    }
}
