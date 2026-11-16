//! The dot pattern shown on the screen and its detection in a color image.
//!
//! White dots sit on a regular grid on black. Three grid corners (top left,
//! top right, bottom left) carry larger *anchor* dots. The bottom right stays
//! normal. The anchors break the four-fold rotational ambiguity of the grid and
//! make the corner correspondences unique, so detection needs no pattern
//! dictionary: blobs, anchors, a seed homography, then RANSAC refinement.

use image::RgbImage;
use nalgebra::{Matrix3, Point2};

use crate::camera::Intrinsics;
use crate::plane::{HomographyFit, apply_homography, distort_pixel, ransac_homography};

mod bias;
mod blobs;
mod edges;
mod grid;

use bias::{Debiased, remove_center_bias};
/// Since: Odyssey Desktop 0.6.5.
pub use bias::{center_bias, corrected_centers};
pub(crate) use blobs::components;
use blobs::{Blob, find_blobs};
/// Since: Odyssey Desktop 0.6.5.
pub use edges::{EdgeCheck, SideSet, check_edges};
use grid::{Matcher, PointIndex, find_seed, refine};

/// The smallest aspect ratio, width over height, the grid is laid out for.
const MIN_ASPECT: f64 = 0.5;
/// The largest aspect ratio the grid is laid out for.
const MAX_ASPECT: f64 = 4.0;
/// The aspect ratio used when the screen's is not a finite number.
const DEFAULT_ASPECT: f64 = 16.0 / 9.0;
/// The fewest dots per grid row and column, whatever the aspect ratio.
const MIN_GRID_SIDE: usize = 4;
/// Smallest distance from the screen edge to the first dot row, as a fraction of the
/// screen height.
const GRID_MARGIN_H: f64 = 0.1;
/// Distance between neighboring dots, as a fraction of the screen height.
const GRID_SPACING_H: f64 = 0.1;
/// Radius of a normal dot as a fraction of the screen height.
const DOT_RADIUS_H: f64 = 0.02;
/// Radius of an anchor dot as a fraction of the screen height.
const ANCHOR_RADIUS_H: f64 = 0.035;
/// The range [`Pattern::with_dot_scale`] clamps its scale to.
const MIN_DOT_SCALE: f64 = 0.5;
const MAX_DOT_SCALE: f64 = 1.4;
/// Thickness of the border ring as a fraction of the screen height.
///
/// Since: Odyssey Desktop 0.6.5.
pub const FRAME_THICKNESS_H: f64 = 0.006;
/// Thickness of the corner brackets' arms as a fraction of the screen height.
const BRACKET_THICKNESS_H: f64 = 0.015;
/// Length of the corner brackets' arms as a fraction of the screen height.
const BRACKET_LENGTH_H: f64 = 0.05;

/// Weights of red, green and blue in the brightness of a pixel, out of 256.
const LUMA_WEIGHTS: [u32; 3] = [77, 150, 29];

/// The fewest dots, blobs or matches that can be taken for the pattern. Below this the grid
/// cannot be told from chance.
const MIN_DOTS: usize = 8;
/// Inliers of refits lie within this share of the dot spacing of the model ...
const INLIER_SPACING_SHARE: f64 = 0.08;
/// ... but not closer than this many pixels, the noise floor of a centroid.
const MIN_INLIER_PX: f64 = 1.0;
/// Dot spacing assumed for the inlier distance when the homography cannot place the center
/// dots.
const FALLBACK_SPACING_PX: f64 = 20.0;
/// RANSAC hypotheses per homography fit.
const RANSAC_ITERATIONS: usize = 200;
/// Fixed seed, so a given image always gives the same detection.
const RANSAC_SEED: u64 = 0x5EED;

/// A filled rectangle of the pattern's border frame. See [`Pattern::frame_rects`].
///
/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRect {
    /// Since: Odyssey Desktop 0.6.5.
    pub x: f64,
    /// Since: Odyssey Desktop 0.6.5.
    pub y: f64,
    /// Since: Odyssey Desktop 0.6.5.
    pub width: f64,
    /// Since: Odyssey Desktop 0.6.5.
    pub height: f64,
}

/// One dot of the pattern.
///
/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    /// Horizontal position, `0` at the left edge and `1` at the right edge.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub u: f64,
    /// Vertical position, `0` at the top edge and `1` at the bottom edge.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub v: f64,
    /// Radius as a fraction of the screen *height*, so a dot is round for any
    /// aspect ratio.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub radius_h: f64,
    /// Since: Odyssey Desktop 0.6.5.
    pub anchor: bool,
}

impl Dot {
    /// Position as the normalized pattern point `(u, v)`.
    pub(crate) fn uv(&self) -> Point2<f64> {
        Point2::new(self.u, self.v)
    }
}

/// The grid of dots for a screen of a given width-to-height ratio. The same
/// description serves every resolution: the renderer scales it to pixels.
///
/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    aspect: f64,
    cols: usize,
    rows: usize,
    dots: Vec<Dot>,
}

impl Pattern {
    /// `aspect` is screen width divided by screen height.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn new(aspect: f64) -> Self {
        let aspect = if aspect.is_finite() {
            aspect.clamp(MIN_ASPECT, MAX_ASPECT)
        } else {
            DEFAULT_ASPECT
        };
        // The epsilon keeps floor() from dropping a row to float error (0.8/0.1).
        let count = |extent: f64| {
            (((extent - 2.0 * GRID_MARGIN_H) / GRID_SPACING_H + 1e-9).floor() as usize + 1)
                .max(MIN_GRID_SIDE)
        };
        let cols = count(aspect);
        let rows = count(1.0);

        // The grid is centered, so the margin is the same on opposite sides.
        let x0 = (aspect - (cols - 1) as f64 * GRID_SPACING_H) / 2.0;
        let y0 = (1.0 - (rows - 1) as f64 * GRID_SPACING_H) / 2.0;
        let mut dots = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let anchor =
                    row == 0 && (col == 0 || col == cols - 1) || row == rows - 1 && col == 0;
                dots.push(Dot {
                    u: (x0 + col as f64 * GRID_SPACING_H) / aspect,
                    v: y0 + row as f64 * GRID_SPACING_H,
                    radius_h: if anchor {
                        ANCHOR_RADIUS_H
                    } else {
                        DOT_RADIUS_H
                    },
                    anchor,
                });
            }
        }
        Self {
            aspect,
            cols,
            rows,
            dots,
        }
    }

    /// Like [`Pattern::new`] with every dot radius multiplied by `scale`,
    /// clamped to `0.5..=1.4` so neighboring dots stay apart. Larger dots help
    /// low-resolution cameras and stress the perspective correction.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn with_dot_scale(aspect: f64, scale: f64) -> Self {
        let mut pattern = Self::new(aspect);
        let scale = if scale.is_finite() {
            scale.clamp(MIN_DOT_SCALE, MAX_DOT_SCALE)
        } else {
            1.0
        };
        for dot in &mut pattern.dots {
            dot.radius_h *= scale;
        }
        pattern
    }

    /// Since: Odyssey Desktop 0.6.5.
    pub fn aspect(&self) -> f64 {
        self.aspect
    }

    /// The border ring and corner brackets as filled white rectangles, in the
    /// pattern's own units: `x` runs `0..aspect` and `y` runs `0..1`, both in
    /// screen heights, so the same numbers draw at any resolution.
    ///
    /// The ring sits on the image's outermost pixels. Where a display crops
    /// the image the ring disappears, which is how [`check_edges`] notices
    /// overscan. The brackets are thicker, so a cropped corner stays easy to
    /// see even when a thin ring blurs away.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn frame_rects(&self) -> Vec<FrameRect> {
        let (width, ring, thickness, length) = (
            self.aspect,
            FRAME_THICKNESS_H,
            BRACKET_THICKNESS_H,
            BRACKET_LENGTH_H,
        );
        let rect = |x, y, w, h| FrameRect {
            x,
            y,
            width: w,
            height: h,
        };
        let mut rects = vec![
            rect(0.0, 0.0, width, ring),
            rect(0.0, 1.0 - ring, width, ring),
            rect(0.0, 0.0, ring, 1.0),
            rect(width - ring, 0.0, ring, 1.0),
        ];
        for (x, y) in [
            (0.0, 0.0),
            (width - length, 0.0),
            (0.0, 1.0 - thickness),
            (width - length, 1.0 - thickness),
        ] {
            rects.push(rect(x, y, length, thickness));
        }
        for (x, y) in [
            (0.0, 0.0),
            (width - thickness, 0.0),
            (0.0, 1.0 - length),
            (width - thickness, 1.0 - length),
        ] {
            rects.push(rect(x, y, thickness, length));
        }
        rects
    }

    /// Grid size as `(columns, rows)`.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn grid(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Dots in row-major order, top row first.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub fn dots(&self) -> &[Dot] {
        &self.dots
    }

    fn index(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }

    fn dot_at(&self, col: usize, row: usize) -> &Dot {
        &self.dots[self.index(col, row)]
    }
}

/// Why no pattern was found.
///
/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectFailure {
    /// Fewer than 8 dot-like blobs in the image.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    NoDots {
        #[doc = "Since: Odyssey Desktop 0.6.5."]
        blobs: usize,
    },
    /// Dots are visible but no anchor triple with a consistent grid.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    NoGrid {
        #[doc = "Since: Odyssey Desktop 0.6.5."]
        blobs: usize,
    },
}

impl std::fmt::Display for DetectFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDots { blobs } => write!(f, "only {blobs} dot-like spots were found"),
            Self::NoGrid { blobs } => write!(
                f,
                "{blobs} dot-like spots were found, too few of them on the pattern's grid"
            ),
        }
    }
}

/// A detected dot paired with its place in the pattern.
///
/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correspondence {
    /// Index into [`Pattern::dots`].
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub index: usize,
    /// Normalized pattern position `(u, v)`.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub pattern: Point2<f64>,
    /// Estimated image of the dot's true center in the pinhole pixel grid,
    /// i.e. with lens distortion and the perspective bias of the dot's
    /// elliptical image removed.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub pixel: Point2<f64>,
    /// Measured centroid of the dot's image, undistorted but not corrected for
    /// perspective. Differs from `pixel` by [`center_bias`].
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub centroid: Point2<f64>,
    /// Centroid in the raw image.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub raw: Point2<f64>,
}

/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectionQuality {
    /// Since: Odyssey Desktop 0.6.5.
    pub matched: usize,
    /// Since: Odyssey Desktop 0.6.5.
    pub expected: usize,
    /// Root-mean-square distance between detected centers and the fitted
    /// homography's prediction, in undistorted pixels.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub rms_px: f64,
    /// Typical pixel distance between neighboring dots.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub spacing_px: f64,
    /// Dot-like blobs found, including ones that matched nothing.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub blobs: usize,
}

impl DetectionQuality {
    /// Since: Odyssey Desktop 0.6.5.
    pub fn completeness(&self) -> f64 {
        self.matched as f64 / self.expected as f64
    }
}

/// Since: Odyssey Desktop 0.6.5.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Since: Odyssey Desktop 0.6.5.
    pub correspondences: Vec<Correspondence>,
    /// Maps normalized pattern coordinates to undistorted pixels.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub homography: Matrix3<f64>,
    /// Screen corners (top left, top right, bottom right, bottom left) in the
    /// raw image, extrapolated from the grid.
    ///
    /// Since: Odyssey Desktop 0.6.5.
    pub quad_px: [Point2<f64>; 4],
    /// Since: Odyssey Desktop 0.6.5.
    pub quality: DetectionQuality,
}

/// A dot matched to the blob that shows it.
#[derive(Debug, Clone, Copy)]
struct Match {
    /// Index into [`Pattern::dots`].
    dot: usize,
    /// Index into the blob list.
    blob: usize,
}

/// The pattern positions `(u, v)` of the matched dots, in the order of `matches`.
fn dot_positions(pattern: &Pattern, matches: &[Match]) -> Vec<Point2<f64>> {
    matches.iter().map(|m| pattern.dots[m.dot].uv()).collect()
}

/// Whether the turn `a -> b -> c` is clockwise or straight. The corners of a projected
/// screen, taken top left, top right, bottom right, bottom left, always turn the other way.
fn turns_wrong_way(a: Point2<f64>, b: Point2<f64>, c: Point2<f64>) -> bool {
    (b - a).perp(&(c - b)) <= 0.0
}

/// Pixel distance from the grid's center dot to its right or lower neighbor, whichever is
/// closer, under the pattern-to-pixel homography `h`.
fn median_spacing(pattern: &Pattern, h: &Matrix3<f64>) -> Option<f64> {
    let (c, r) = (pattern.cols / 2, pattern.rows / 2);
    let d = |c, r| apply_homography(h, pattern.dot_at(c, r).uv());
    let center = d(c, r)?;
    let right = (d(c + 1, r)? - center).norm();
    let down = (d(c, r + 1)? - center).norm();
    Some(right.min(down))
}

/// Inlier distance in pixels for refits under `h`: a share of the dot spacing, so it scales
/// with how large the screen appears, and never below the centroid noise floor.
fn inlier_threshold(pattern: &Pattern, h: &Matrix3<f64>) -> f64 {
    let spacing = median_spacing(pattern, h).unwrap_or(FALLBACK_SPACING_PX);
    (INLIER_SPACING_SHARE * spacing).max(MIN_INLIER_PX)
}

/// RANSAC homography fit of `src` (pattern positions) to `dst` (pixels).
fn robust_fit(src: &[Point2<f64>], dst: &[Point2<f64>], threshold: f64) -> Option<HomographyFit> {
    ransac_homography(src, dst, threshold, RANSAC_ITERATIONS, RANSAC_SEED)
}

/// Finds the pattern in `image`, a raw (distorted) color frame taken with
/// `intrinsics`.
///
/// # Errors
///
/// [`DetectFailure::NoDots`] when too few blobs are found, [`DetectFailure::NoGrid`] when no
/// anchor triple yields a grid or the screen corners do not project to finite pixels.
///
/// # Since
/// Odyssey Desktop 0.6.5.
pub fn detect(
    pattern: &Pattern,
    image: &RgbImage,
    intrinsics: &Intrinsics,
) -> Result<Detection, DetectFailure> {
    let blobs = find_blobs(image, intrinsics);
    let blob_count = blobs.len();
    if blob_count < MIN_DOTS {
        return Err(DetectFailure::NoDots { blobs: blob_count });
    }
    let no_grid = DetectFailure::NoGrid { blobs: blob_count };

    let index = PointIndex::new(&blobs);
    let matcher = Matcher {
        pattern,
        blobs: &blobs,
        index: &index,
    };
    let seed = find_seed(&matcher).ok_or(no_grid)?;
    let (h, matches) = refine(&matcher, seed).ok_or(no_grid)?;
    let threshold = inlier_threshold(pattern, &h);
    let Debiased {
        h,
        matches,
        centers,
    } = remove_center_bias(pattern, &blobs, h, matches, threshold);

    let correspondences = correspondences(pattern, &blobs, &matches, &centers);
    let quality = detection_quality(pattern, &h, &correspondences, blob_count);
    let quad_px = screen_quad(&h, intrinsics).ok_or(no_grid)?;
    Ok(Detection {
        correspondences,
        homography: h,
        quad_px,
        quality,
    })
}

/// The matches as [`Correspondence`]s ordered by dot index. `centers` holds the corrected
/// center of each match, in the order of `matches`.
fn correspondences(
    pattern: &Pattern,
    blobs: &[Blob],
    matches: &[Match],
    centers: &[Point2<f64>],
) -> Vec<Correspondence> {
    let mut order: Vec<usize> = (0..matches.len()).collect();
    order.sort_by_key(|&i| matches[i].dot);
    order
        .into_iter()
        .map(|i| {
            let Match { dot, blob } = matches[i];
            Correspondence {
                index: dot,
                pattern: pattern.dots[dot].uv(),
                pixel: centers[i],
                centroid: blobs[blob].undistorted,
                raw: blobs[blob].raw,
            }
        })
        .collect()
}

fn detection_quality(
    pattern: &Pattern,
    h: &Matrix3<f64>,
    correspondences: &[Correspondence],
    blob_count: usize,
) -> DetectionQuality {
    let squared_error: f64 = correspondences
        .iter()
        .filter_map(|c| apply_homography(h, c.pattern).map(|p| (p - c.pixel).norm_squared()))
        .sum();
    DetectionQuality {
        matched: correspondences.len(),
        expected: pattern.dots.len(),
        rms_px: (squared_error / correspondences.len() as f64).sqrt(),
        spacing_px: median_spacing(pattern, h).unwrap_or(0.0),
        blobs: blob_count,
    }
}

/// The screen's corners (top left, top right, bottom right, bottom left) in the raw image.
fn screen_quad(h: &Matrix3<f64>, intrinsics: &Intrinsics) -> Option<[Point2<f64>; 4]> {
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let mut quad = [Point2::origin(); 4];
    for (slot, (u, v)) in quad.iter_mut().zip(corners) {
        *slot = distort_pixel(intrinsics, apply_homography(h, Point2::new(u, v))?);
        if !slot.x.is_finite() || !slot.y.is_finite() {
            return None;
        }
    }
    Some(quad)
}

#[cfg(test)]
mod tests;
