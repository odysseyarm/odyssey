//! The dot pattern shown on the screen and its detection in a color image.
//!
//! White dots sit on a regular grid on black. Three grid corners (top left,
//! top right, bottom left) carry larger *anchor* dots; the bottom right stays
//! normal. The anchors break the four-fold rotational ambiguity of the grid and
//! make the corner correspondences unique, so detection needs no pattern
//! dictionary: blobs, anchors, a seed homography, then robust refinement.

use std::collections::HashMap;

use image::RgbImage;
use nalgebra::{Matrix3, Point2, Vector2, Vector3};

use crate::camera::Intrinsics;
use crate::plane::{
    apply_homography, distort_pixel, fit_homography, ransac_homography, undistort_pixel,
};

/// Distance from the screen edge to the first dot row, as a fraction of the
/// screen height.
const GRID_MARGIN_H: f64 = 0.1;
/// Distance between neighboring dots, as a fraction of the screen height.
const GRID_SPACING_H: f64 = 0.1;
const DOT_RADIUS_H: f64 = 0.02;
const ANCHOR_RADIUS_H: f64 = 0.035;
/// Thickness of the border ring as a fraction of the screen height.
pub const FRAME_THICKNESS_H: f64 = 0.006;
const BRACKET_THICKNESS_H: f64 = 0.015;
const BRACKET_LENGTH_H: f64 = 0.05;

/// A filled rectangle of the pattern's border frame; see [`Pattern::frame_rects`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One dot of the pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    /// Horizontal position, `0` at the left edge and `1` at the right edge.
    pub u: f64,
    /// Vertical position, `0` at the top edge and `1` at the bottom edge.
    pub v: f64,
    /// Radius as a fraction of the screen *height*, so a dot is round for any
    /// aspect ratio.
    pub radius_h: f64,
    pub anchor: bool,
}

/// The grid of dots for a screen of a given width-to-height ratio. The same
/// description serves every resolution: the renderer scales it to pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    aspect: f64,
    cols: usize,
    rows: usize,
    dots: Vec<Dot>,
}

impl Pattern {
    /// `aspect` is screen width divided by screen height.
    pub fn new(aspect: f64) -> Self {
        let aspect = if aspect.is_finite() {
            aspect.clamp(0.5, 4.0)
        } else {
            16.0 / 9.0
        };
        // Epsilons keep floor() from dropping a row to float error (0.8/0.1).
        let count = |extent: f64| {
            (((extent - 2.0 * GRID_MARGIN_H) / GRID_SPACING_H + 1e-9).floor() as usize + 1).max(4)
        };
        let cols = count(aspect);
        let rows = count(1.0);

        let x0 = (aspect - (cols - 1) as f64 * GRID_SPACING_H) / 2.0;
        let y0 = (1.0 - (rows - 1) as f64 * GRID_SPACING_H) / 2.0;
        let mut dots = Vec::with_capacity(cols * rows);
        for r in 0..rows {
            for c in 0..cols {
                let anchor =
                    (r == 0 && c == 0) || (r == 0 && c == cols - 1) || (r == rows - 1 && c == 0);
                dots.push(Dot {
                    u: (x0 + c as f64 * GRID_SPACING_H) / aspect,
                    v: y0 + r as f64 * GRID_SPACING_H,
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
    pub fn with_dot_scale(aspect: f64, scale: f64) -> Self {
        let mut pattern = Self::new(aspect);
        let scale = if scale.is_finite() {
            scale.clamp(0.5, 1.4)
        } else {
            1.0
        };
        for dot in &mut pattern.dots {
            dot.radius_h *= scale;
        }
        pattern
    }

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
    pub fn frame_rects(&self) -> Vec<FrameRect> {
        let (w, t, bt, bl) = (
            self.aspect,
            FRAME_THICKNESS_H,
            BRACKET_THICKNESS_H,
            BRACKET_LENGTH_H,
        );
        let rect = |x, y, width, height| FrameRect {
            x,
            y,
            width,
            height,
        };
        let mut rects = vec![
            rect(0.0, 0.0, w, t),
            rect(0.0, 1.0 - t, w, t),
            rect(0.0, 0.0, t, 1.0),
            rect(w - t, 0.0, t, 1.0),
        ];
        for (x, y) in [
            (0.0, 0.0),
            (w - bl, 0.0),
            (0.0, 1.0 - bt),
            (w - bl, 1.0 - bt),
        ] {
            rects.push(rect(x, y, bl, bt));
        }
        for (x, y) in [
            (0.0, 0.0),
            (w - bt, 0.0),
            (0.0, 1.0 - bl),
            (w - bt, 1.0 - bl),
        ] {
            rects.push(rect(x, y, bt, bl));
        }
        rects
    }

    /// Grid size as `(columns, rows)`.
    pub fn grid(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Dots in row-major order, top row first.
    pub fn dots(&self) -> &[Dot] {
        &self.dots
    }

    fn index(&self, col: usize, row: usize) -> usize {
        row * self.cols + col
    }
}

/// Why no pattern was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectFailure {
    /// Fewer than a handful of dot-like blobs in the image.
    NoDots { blobs: usize },
    /// Dots are visible but no anchor triple with a consistent grid.
    NoGrid { blobs: usize },
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correspondence {
    /// Index into [`Pattern::dots`].
    pub index: usize,
    /// Normalized pattern position `(u, v)`.
    pub pattern: Point2<f64>,
    /// Estimated image of the dot's true center in the pinhole pixel grid,
    /// i.e. with lens distortion and the perspective bias of the dot's
    /// elliptical image removed.
    pub pixel: Point2<f64>,
    /// Measured centroid of the dot's image, undistorted but not corrected for
    /// perspective. Differs from `pixel` by [`center_bias`].
    pub centroid: Point2<f64>,
    /// Centroid in the raw image.
    pub raw: Point2<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectionQuality {
    pub matched: usize,
    pub expected: usize,
    /// Root-mean-square distance between detected centers and the fitted
    /// homography's prediction, in undistorted pixels.
    pub rms_px: f64,
    /// Typical pixel distance between neighboring dots.
    pub spacing_px: f64,
    /// Dot-like blobs found, including ones that matched nothing.
    pub blobs: usize,
}

impl DetectionQuality {
    pub fn completeness(&self) -> f64 {
        self.matched as f64 / self.expected as f64
    }
}

#[derive(Debug, Clone)]
pub struct Detection {
    pub correspondences: Vec<Correspondence>,
    /// Maps normalized pattern coordinates to undistorted pixels.
    pub homography: Matrix3<f64>,
    /// Screen corners (top left, top right, bottom right, bottom left) in the
    /// raw image, extrapolated from the grid.
    pub quad_px: [Point2<f64>; 4],
    pub quality: DetectionQuality,
}

/// Connected region of bright pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Component {
    pub area: u32,
    /// Centroid weighted by the per-pixel weight, in pixel-center coordinates.
    pub center: Point2<f64>,
    pub bbox_w: u32,
    pub bbox_h: u32,
    pub touches_border: bool,
    /// Ratio of the minor to the major axis of the pixel distribution.
    pub axis_ratio: f64,
}

/// Labels 4-connected foreground regions. `weight(x, y)` is positive for
/// foreground pixels and also weights the centroid, so antialiased edges
/// contribute proportionally to their brightness.
pub(crate) fn components(
    width: u32,
    height: u32,
    weight: impl Fn(u32, u32) -> f64,
) -> Vec<Component> {
    let (w, h) = (width as usize, height as usize);
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut out = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            if seen[sy * w + sx] {
                continue;
            }
            seen[sy * w + sx] = true;
            if weight(sx as u32, sy as u32) <= 0.0 {
                continue;
            }
            stack.push((sx, sy));
            let (mut area, mut wsum, mut wx, mut wy) = (0u32, 0.0, 0.0, 0.0);
            let (mut sxx, mut sxy, mut syy, mut mx, mut my) = (0.0, 0.0, 0.0, 0.0, 0.0);
            let (mut x0, mut x1, mut y0, mut y1) = (sx, sx, sy, sy);
            while let Some((x, y)) = stack.pop() {
                let wt = weight(x as u32, y as u32);
                area += 1;
                wsum += wt;
                wx += wt * x as f64;
                wy += wt * y as f64;
                mx += x as f64;
                my += y as f64;
                sxx += (x * x) as f64;
                sxy += (x * y) as f64;
                syy += (y * y) as f64;
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
                let neighbors = [
                    (x > 0).then(|| (x - 1, y)),
                    (x + 1 < w).then(|| (x + 1, y)),
                    (y > 0).then(|| (x, y - 1)),
                    (y + 1 < h).then(|| (x, y + 1)),
                ];
                for (nx, ny) in neighbors.into_iter().flatten() {
                    if !seen[ny * w + nx] {
                        seen[ny * w + nx] = true;
                        if weight(nx as u32, ny as u32) > 0.0 {
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            let n = area as f64;
            let (cx, cy) = (mx / n, my / n);
            let (vxx, vxy, vyy) = (sxx / n - cx * cx, sxy / n - cx * cy, syy / n - cy * cy);
            let half_trace = (vxx + vyy) / 2.0;
            let disc = (((vxx - vyy) / 2.0).powi(2) + vxy * vxy).sqrt();
            let (major, minor) = ((half_trace + disc).max(1e-12), (half_trace - disc).max(0.0));
            out.push(Component {
                area,
                center: Point2::new(wx / wsum, wy / wsum),
                bbox_w: (x1 - x0 + 1) as u32,
                bbox_h: (y1 - y0 + 1) as u32,
                touches_border: x0 == 0 || y0 == 0 || x1 + 1 == w || y1 + 1 == h,
                axis_ratio: (minor / major).sqrt(),
            });
        }
    }
    out
}

fn luma(image: &RgbImage) -> Vec<u8> {
    image
        .pixels()
        .map(|p| ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8) as u8)
        .collect()
}

/// Integral image for windowed means.
struct Integral {
    w: usize,
    sums: Vec<u64>,
}

impl Integral {
    fn new(gray: &[u8], w: usize, h: usize) -> Self {
        let stride = w + 1;
        let mut sums = vec![0u64; stride * (h + 1)];
        for y in 0..h {
            let mut row = 0u64;
            for x in 0..w {
                row += gray[y * w + x] as u64;
                sums[(y + 1) * stride + x + 1] = sums[y * stride + x + 1] + row;
            }
        }
        Self { w: stride, sums }
    }

    fn mean(&self, x: usize, y: usize, half: usize, width: usize, height: usize) -> f64 {
        let (x0, x1) = (x.saturating_sub(half), (x + half + 1).min(width));
        let (y0, y1) = (y.saturating_sub(half), (y + half + 1).min(height));
        let s = self.sums[y1 * self.w + x1] + self.sums[y0 * self.w + x0]
            - self.sums[y0 * self.w + x1]
            - self.sums[y1 * self.w + x0];
        s as f64 / ((x1 - x0) * (y1 - y0)) as f64
    }
}

/// A dot-shaped blob in the raw image.
#[derive(Debug, Clone, Copy)]
struct Blob {
    raw: Point2<f64>,
    undistorted: Point2<f64>,
    area: f64,
}

/// Finds bright round blobs. A pixel is foreground when it exceeds the mean of
/// its neighborhood by a margin tied to the image contrast, which keeps dots
/// detectable across screen brightness and room lighting without a global
/// threshold.
fn find_blobs(image: &RgbImage, intr: &Intrinsics) -> Vec<Blob> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let gray = luma(image);
    let mut hist = [0u32; 256];
    for &g in &gray {
        hist[g as usize] += 1;
    }
    let percentile = |p: f64| {
        let target = (p * gray.len() as f64) as u32;
        let mut acc = 0;
        for (value, &count) in hist.iter().enumerate() {
            acc += count;
            if acc >= target {
                return value as f64;
            }
        }
        255.0
    };
    let range = percentile(0.995) - percentile(0.05);
    let margin = (0.25 * range).max(12.0);
    let half = (h / 24).max(7);
    let integral = Integral::new(&gray, w, h);

    let weight = |x: u32, y: u32| {
        let g = gray[y as usize * w + x as usize] as f64;
        let local = integral.mean(x as usize, y as usize, half, w, h);
        if g > local + margin { g - local } else { 0.0 }
    };

    let max_area = (0.08 * h as f64).powi(2);
    components(w as u32, h as u32, weight)
        .into_iter()
        .filter(|c| {
            let a = c.area as f64;
            let (bw, bh) = (c.bbox_w as f64, c.bbox_h as f64);
            let tiny = bw.min(bh) < 4.0;
            let fill = a / (bw * bh);
            !c.touches_border
                && a >= 5.0
                && a <= max_area
                && (0.4..=2.5).contains(&(bw / bh))
                && c.axis_ratio >= 0.25
                && (tiny || (0.5..=0.95).contains(&fill))
        })
        .filter_map(|c| {
            let raw = c.center;
            let undistorted = undistort_pixel(intr, raw)?;
            Some(Blob {
                raw,
                undistorted,
                area: c.area as f64,
            })
        })
        .collect()
}

/// Cell-bucketed lookup of blob centers.
struct PointIndex {
    cell: f64,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl PointIndex {
    fn new(blobs: &[Blob], cell: f64) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, b) in blobs.iter().enumerate() {
            cells
                .entry(Self::key(cell, b.undistorted))
                .or_default()
                .push(i as u32);
        }
        Self { cell, cells }
    }

    fn key(cell: f64, p: Point2<f64>) -> (i32, i32) {
        ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32)
    }

    /// Blobs within `radius` of `p`, with their distances.
    fn within<'a>(
        &'a self,
        blobs: &'a [Blob],
        p: Point2<f64>,
        radius: f64,
    ) -> impl Iterator<Item = (usize, f64)> + 'a {
        let span = (radius / self.cell).ceil() as i32;
        let (kx, ky) = Self::key(self.cell, p);
        (-span..=span)
            .flat_map(move |dy| (-span..=span).map(move |dx| (kx + dx, ky + dy)))
            .filter_map(|k| self.cells.get(&k))
            .flatten()
            .filter_map(move |&i| {
                let d = (blobs[i as usize].undistorted - p).norm();
                (d <= radius).then_some((i as usize, d))
            })
    }
}

struct Evaluation {
    /// `(dot index, blob index)`.
    matches: Vec<(usize, usize)>,
    /// Match count minus a small penalty for sloppy matches, so equal counts
    /// prefer the tighter fit.
    score: f64,
}

struct Matcher<'a> {
    pattern: &'a Pattern,
    blobs: &'a [Blob],
    index: &'a PointIndex,
}

impl Matcher<'_> {
    /// A cheap necessary condition for [`evaluate`](Self::evaluate) to find much:
    /// a sparse sample of the dots, predicted with `h`, must mostly land on
    /// blobs. It lets the seed search throw out the many wrong hypotheses
    /// without predicting and matching all the dots for each.
    fn plausible(&self, h: &Matrix3<f64>, gate: f64) -> bool {
        const STRIDE: usize = 3;
        let p = self.pattern;
        let at = |c: usize, r: usize| {
            let d = &p.dots[p.index(c, r)];
            apply_homography(h, Point2::new(d.u, d.v))
        };
        let (mut tried, mut hit) = (0usize, 0usize);
        for r in (0..p.rows).step_by(STRIDE) {
            for c in (0..p.cols).step_by(STRIDE) {
                let (c_next, r_next) = (
                    if c + 1 < p.cols { c + 1 } else { c - 1 },
                    if r + 1 < p.rows { r + 1 } else { r - 1 },
                );
                let (Some(here), Some(across), Some(down)) =
                    (at(c, r), at(c_next, r), at(c, r_next))
                else {
                    return false;
                };
                let spacing = (across - here).norm().min((down - here).norm());
                tried += 1;
                if self
                    .index
                    .within(self.blobs, here, gate * spacing)
                    .next()
                    .is_some()
                {
                    hit += 1;
                }
            }
        }
        // A third is enough: a partly hidden screen still has to be tried.
        hit * 10 >= tried * 3
    }

    /// Predicts every dot with `h`, then matches each to the nearest blob
    /// within `gate` times the local dot spacing, one blob per dot.
    fn evaluate(&self, h: &Matrix3<f64>, gate: f64) -> Option<Evaluation> {
        let p = self.pattern;
        let predicted: Vec<Point2<f64>> = p
            .dots
            .iter()
            .map(|d| apply_homography(h, Point2::new(d.u, d.v)))
            .collect::<Option<_>>()?;

        // A projection of a screen is convex and keeps the winding order.
        let corner = |c, r| predicted[p.index(c, r)];
        let quad = [
            corner(0, 0),
            corner(p.cols - 1, 0),
            corner(p.cols - 1, p.rows - 1),
            corner(0, p.rows - 1),
        ];
        for i in 0..4 {
            let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
            if (b - a).perp(&(c - b)) <= 0.0 {
                return None;
            }
        }

        let mut candidates = Vec::new();
        for r in 0..p.rows {
            for c in 0..p.cols {
                let i = p.index(c, r);
                let here = predicted[i];
                let mut spacing = f64::INFINITY;
                if c > 0 {
                    spacing = spacing.min((predicted[p.index(c - 1, r)] - here).norm());
                }
                if c + 1 < p.cols {
                    spacing = spacing.min((predicted[p.index(c + 1, r)] - here).norm());
                }
                if r > 0 {
                    spacing = spacing.min((predicted[p.index(c, r - 1)] - here).norm());
                }
                if r + 1 < p.rows {
                    spacing = spacing.min((predicted[p.index(c, r + 1)] - here).norm());
                }
                let radius = gate * spacing;
                if let Some((blob, d)) = self
                    .index
                    .within(self.blobs, here, radius)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                {
                    candidates.push((d / radius, i, blob));
                }
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut taken = vec![false; self.blobs.len()];
        let mut matches = Vec::new();
        let mut penalty = 0.0;
        for (rel, dot, blob) in candidates {
            if !taken[blob] {
                taken[blob] = true;
                matches.push((dot, blob));
                penalty += rel;
            }
        }
        let score = matches.len() as f64 - 0.01 * penalty;
        Some(Evaluation { matches, score })
    }
}

const SEED_GATE: f64 = 0.3;
const REFINE_GATE: f64 = 0.35;
const MAX_ANCHOR_CANDIDATES: usize = 16;

/// Blobs whose area clearly exceeds that of their nearest neighbors. Comparing
/// against neighbors instead of the global median keeps perspective, which
/// shrinks far dots, from promoting near dots to anchors.
fn anchor_candidates(blobs: &[Blob]) -> Vec<usize> {
    let mut scored = Vec::new();
    for (i, b) in blobs.iter().enumerate() {
        let mut near: Vec<(f64, f64)> = blobs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, o)| ((o.undistorted - b.undistorted).norm(), o.area))
            .collect();
        if near.len() < 4 {
            continue;
        }
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut areas: Vec<f64> = near.iter().take(8).map(|n| n.1).collect();
        areas.sort_by(|a, b| a.total_cmp(b));
        let ratio = b.area / areas[areas.len() / 2];
        if (1.9..=5.0).contains(&ratio) {
            scored.push((ratio, i));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(MAX_ANCHOR_CANDIDATES)
        .map(|s| s.1)
        .collect()
}

/// Whether the three corner candidates `[tl, tr, bl]` have dots next to them along
/// the grid's edges. This is the lattice test that separates a pattern's corners
/// from a bright spot that happens to stand out: a spot has no dot in two
/// perpendicular directions nearby. One corner direction may be missing.
fn corner_neighbors_present(
    index: &PointIndex,
    blobs: &[Blob],
    [a, b, c]: [Point2<f64>; 3],
    pattern: &Pattern,
) -> bool {
    let across = (b - a) / (pattern.cols - 1) as f64;
    let down = (c - a) / (pattern.rows - 1) as f64;
    // Each corner and the two edge directions that leave it into the grid.
    let edges = [
        (a, across),
        (a, down),
        (b, -across),
        (b, down),
        (c, across),
        (c, -down),
    ];
    edges
        .iter()
        .filter(|(corner, step)| dot_along(index, blobs, *corner, *step))
        .count()
        >= 5
}

/// Whether a dot lies roughly in direction `step` from `corner`, `step` being the
/// grid spacing there. Perspective changes the spacing along an edge by a large
/// factor and bends directions a little, so this accepts a dot at any plausible
/// distance within a generous cone rather than at one predicted position.
fn dot_along(index: &PointIndex, blobs: &[Blob], corner: Point2<f64>, step: Vector2<f64>) -> bool {
    const NEAREST: f64 = 0.25;
    const FARTHEST: f64 = 3.5;
    const MIN_COS: f64 = 0.8;
    let reach = step.norm();
    index.within(blobs, corner, FARTHEST * reach).any(|(i, d)| {
        d >= NEAREST * reach && (blobs[i].undistorted - corner).dot(&step) >= MIN_COS * d * reach
    })
}

/// Finds the pattern in `image`, a raw (distorted) color frame taken with
/// `intrinsics`.
pub fn detect(
    pattern: &Pattern,
    image: &RgbImage,
    intrinsics: &Intrinsics,
) -> Result<Detection, DetectFailure> {
    let blobs = find_blobs(image, intrinsics);
    let n = blobs.len();
    if n < 8 {
        return Err(DetectFailure::NoDots { blobs: n });
    }
    let index = PointIndex::new(&blobs, 16.0);
    let matcher = Matcher {
        pattern,
        blobs: &blobs,
        index: &index,
    };
    let total = pattern.dots.len();
    let anchors = anchor_candidates(&blobs);

    let corner_uv = |c, r| {
        let d = &pattern.dots[pattern.index(c, r)];
        Point2::new(d.u, d.v)
    };
    let (tl_uv, tr_uv, bl_uv, br_uv) = (
        corner_uv(0, 0),
        corner_uv(pattern.cols - 1, 0),
        corner_uv(0, pattern.rows - 1),
        corner_uv(pattern.cols - 1, pattern.rows - 1),
    );
    let expected_ratio = (pattern.cols - 1) as f64 / (pattern.rows - 1) as f64;

    let mut best: Option<(f64, Matrix3<f64>)> = None;
    // Triples are tried in order of their lowest-ranked member, so the best-ranked
    // anchors are exhausted first and a clean view stops early.
    'seed: for newest in 0..anchors.len() {
        for (i, j, l) in (0..=newest)
            .flat_map(|i| (0..=newest).flat_map(move |j| (0..=newest).map(move |l| (i, j, l))))
        {
            if i.max(j).max(l) != newest {
                continue;
            }
            let (tl, tr, bl) = (anchors[i], anchors[j], anchors[l]);
            if tl == tr || tl == bl || tr == bl {
                continue;
            }
            let (a, b, c) = (
                blobs[tl].undistorted,
                blobs[tr].undistorted,
                blobs[bl].undistorted,
            );
            let (across, down) = (b - a, c - a);
            if across.perp(&down) <= 0.0 {
                continue;
            }
            // Foreshortening at steep viewing angles squeezes one side of
            // the grid, so this only rejects triples that cannot be corners.
            let ratio = across.norm() / down.norm();
            if ratio < expected_ratio / 6.0 || ratio > expected_ratio * 6.0 {
                continue;
            }
            // A corner dot has dots beside it along both edges, at the spacing
            // the triple implies. Clutter that merely looks like a corner has
            // none, which keeps a wide anchor pool cheap.
            if !corner_neighbors_present(&index, &blobs, [a, b, c], pattern) {
                continue;
            }
            // The fourth corner is not where an affine guess puts it once
            // perspective is strong, so any blob that keeps the quad convex
            // is a candidate; the grid evaluation picks the real one.
            for (br, blob) in blobs.iter().enumerate() {
                if br == tl || br == tr || br == bl {
                    continue;
                }
                let d = blob.undistorted;
                if (b - a).perp(&(d - b)) <= 0.0
                    || (d - b).perp(&(c - d)) <= 0.0
                    || (c - d).perp(&(a - c)) <= 0.0
                {
                    continue;
                }
                // The fourth corner is a corner dot as well: dots lie along both of
                // the edges that leave it, toward the other two corners.
                let to_c = (c - d) / (pattern.cols - 1) as f64;
                let to_b = (b - d) / (pattern.rows - 1) as f64;
                if !dot_along(&index, &blobs, d, to_c) || !dot_along(&index, &blobs, d, to_b) {
                    continue;
                }
                let src = [tl_uv, tr_uv, bl_uv, br_uv];
                let dst = [a, b, c, blobs[br].undistorted];
                let Some(h) = fit_homography(&src, &dst) else {
                    continue;
                };
                if !matcher.plausible(&h, SEED_GATE) {
                    continue;
                }
                let Some(eval) = matcher.evaluate(&h, SEED_GATE) else {
                    continue;
                };
                if best.as_ref().is_none_or(|(s, _)| eval.score > *s) {
                    best = Some((eval.score, h));
                }
                // Nearly every dot matched: no other seed can be meaningfully
                // better, and searching on is most of the cost.
                if eval.score >= 0.9 * total as f64 {
                    break 'seed;
                }
            }
        }
    }
    let Some((seed_score, mut h)) = best else {
        return Err(DetectFailure::NoGrid { blobs: n });
    };
    if seed_score < (0.4 * total as f64).max(8.0) {
        return Err(DetectFailure::NoGrid { blobs: n });
    }

    // Refinement: refit on the matches and rematch with the improved model,
    // which also pulls in dots the four-corner seed predicted loosely.
    let mut matches = Vec::new();
    for _ in 0..4 {
        let Some(eval) = matcher.evaluate(&h, REFINE_GATE) else {
            break;
        };
        let src: Vec<Point2<f64>> = eval
            .matches
            .iter()
            .map(|(d, _)| Point2::new(pattern.dots[*d].u, pattern.dots[*d].v))
            .collect();
        let dst: Vec<Point2<f64>> = eval
            .matches
            .iter()
            .map(|(_, b)| blobs[*b].undistorted)
            .collect();
        let spacing = median_spacing(pattern, &h).unwrap_or(20.0);
        let threshold = (0.08 * spacing).max(1.0);
        let Some(fit) = ransac_homography(&src, &dst, threshold, 200, 0x5EED) else {
            break;
        };
        let changed = fit.inliers.len() != matches.len();
        h = fit.h;
        matches = fit.inliers.iter().map(|&i| eval.matches[i]).collect();
        if !changed {
            break;
        }
    }
    if matches.len() < (0.4 * total as f64) as usize || matches.len() < 8 {
        return Err(DetectFailure::NoGrid { blobs: n });
    }

    let spacing = median_spacing(pattern, &h).unwrap_or(20.0);
    let Debiased {
        h,
        matches,
        centers,
    } = remove_center_bias(pattern, &blobs, h, matches, (0.08 * spacing).max(1.0));

    let mut order: Vec<usize> = (0..matches.len()).collect();
    order.sort_by_key(|&i| matches[i].0);
    let correspondences: Vec<Correspondence> = order
        .into_iter()
        .map(|i| {
            let (d, b) = matches[i];
            Correspondence {
                index: d,
                pattern: Point2::new(pattern.dots[d].u, pattern.dots[d].v),
                pixel: centers[i],
                centroid: blobs[b].undistorted,
                raw: blobs[b].raw,
            }
        })
        .collect();

    let sq: f64 = correspondences
        .iter()
        .filter_map(|c| apply_homography(&h, c.pattern).map(|p| (p - c.pixel).norm_squared()))
        .sum();
    let quality = DetectionQuality {
        matched: correspondences.len(),
        expected: total,
        rms_px: (sq / correspondences.len() as f64).sqrt(),
        spacing_px: median_spacing(pattern, &h).unwrap_or(0.0),
        blobs: n,
    };

    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let mut quad_px = [Point2::origin(); 4];
    for (slot, (u, v)) in quad_px.iter_mut().zip(corners) {
        let p =
            apply_homography(&h, Point2::new(u, v)).ok_or(DetectFailure::NoGrid { blobs: n })?;
        *slot = distort_pixel(intrinsics, p);
    }
    Ok(Detection {
        correspondences,
        homography: h,
        quad_px,
        quality,
    })
}

/// Passes of [`remove_center_bias`]. Each pass refits the homography the next
/// one linearizes around; the correction converges geometrically, so three
/// passes reach well below the centroid noise.
const BIAS_PASSES: usize = 3;
/// The correction has converged once no dot's offset moves by more than this.
const BIAS_CONVERGED_PX: f64 = 0.01;

/// Offset between the center of a dot's elliptical image and the image of the
/// dot's true center, in pixels, for a pattern-to-pixel homography
/// `unit_to_pixel`.
///
/// A circle seen obliquely images to an ellipse, and perspective foreshortens
/// its far half more than its near half, so the ellipse center (which is where
/// a centroid lands) is not the image of the circle's center. The ellipse
/// center is the pole of the line at infinity with respect to the projected
/// conic. With `M` mapping the unit circle to the image, that pole is
/// `M diag(1, 1, -1) M^T (0, 0, 1)^T`.
///
/// The circle lives in physical pattern coordinates (units of screen height,
/// so the dot is round); the homography takes normalized `(u, v)`, which
/// stretch `x` by the aspect ratio.
///
/// Lens distortion is deliberately not part of this model. The centroids are
/// undistorted point-wise before the correction, which treats a dot as a point
/// for the lens even though its outline is bent slightly; at dot sizes of
/// 1-2% of the screen height that second-order term stays far below the
/// perspective term.
pub fn center_bias(
    pattern: &Pattern,
    dot: usize,
    unit_to_pixel: &Matrix3<f64>,
) -> Option<Vector2<f64>> {
    let d = pattern.dots.get(dot)?;
    let (x, y) = (d.u * pattern.aspect, d.v);
    let physical =
        unit_to_pixel * Matrix3::new(1.0 / pattern.aspect, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0);
    let m = physical * Matrix3::new(d.radius_h, 0.0, x, 0.0, d.radius_h, y, 0.0, 0.0, 1.0);
    let bottom = m.row(2).transpose();
    let pole = m * Matrix3::from_diagonal(&Vector3::new(1.0, 1.0, -1.0)) * bottom;
    if pole.z.abs() < 1e-12 {
        return None;
    }
    let ellipse_center = Point2::new(pole.x / pole.z, pole.y / pole.z);
    let image_of_center = apply_homography(&physical, Point2::new(x, y))?;
    Some(ellipse_center - image_of_center)
}

/// Estimates the image of each dot's true center from its measured centroid by
/// subtracting [`center_bias`] under `unit_to_pixel`. `measured` pairs a dot
/// index with its centroid in undistorted pixels. `None` if the homography
/// leaves any dot degenerate.
pub fn corrected_centers(
    pattern: &Pattern,
    unit_to_pixel: &Matrix3<f64>,
    measured: &[(usize, Point2<f64>)],
) -> Option<Vec<Point2<f64>>> {
    measured
        .iter()
        .map(|&(dot, p)| Some(p - center_bias(pattern, dot, unit_to_pixel)?))
        .collect()
}

/// Homography and matches after bias removal.
struct Debiased {
    h: Matrix3<f64>,
    /// `(dot index, blob index)` pairs that survived the refits.
    matches: Vec<(usize, usize)>,
    /// Corrected centers, in the order of `matches`.
    centers: Vec<Point2<f64>>,
}

/// Refits the homography on bias-corrected centers until the correction stops
/// changing.
fn remove_center_bias(
    pattern: &Pattern,
    blobs: &[Blob],
    mut h: Matrix3<f64>,
    mut matches: Vec<(usize, usize)>,
    threshold: f64,
) -> Debiased {
    let mut centers: Vec<Point2<f64>> =
        matches.iter().map(|&(_, b)| blobs[b].undistorted).collect();
    let mut previous = vec![Vector2::zeros(); matches.len()];
    for _ in 0..BIAS_PASSES {
        let Some(bias) = matches
            .iter()
            .map(|&(d, _)| center_bias(pattern, d, &h))
            .collect::<Option<Vec<_>>>()
        else {
            break;
        };
        let change = bias
            .iter()
            .zip(&previous)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0, f64::max);
        if change < BIAS_CONVERGED_PX {
            break;
        }
        let src: Vec<_> = matches
            .iter()
            .map(|&(d, _)| Point2::new(pattern.dots[d].u, pattern.dots[d].v))
            .collect();
        let dst: Vec<_> = matches
            .iter()
            .zip(&bias)
            .map(|(&(_, b), off)| blobs[b].undistorted - off)
            .collect();
        let Some(fit) = ransac_homography(&src, &dst, threshold, 200, 0x5EED) else {
            break;
        };
        h = fit.h;
        centers = fit.inliers.iter().map(|&i| dst[i]).collect();
        previous = fit.inliers.iter().map(|&i| bias[i]).collect();
        matches = fit.inliers.iter().map(|&i| matches[i]).collect();
    }
    Debiased {
        h,
        matches,
        centers,
    }
}

/// Which sides of the screen image. The ring on a side is the pattern's
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

fn luma_at(image: &RgbImage, x: u32, y: u32) -> f64 {
    let p = image.get_pixel(x, y);
    (77.0 * f64::from(p[0]) + 150.0 * f64::from(p[1]) + 29.0 * f64::from(p[2])) / 256.0
}

/// Brightest pixel in the 3x3 block around `p`; absorbs a pixel of
/// registration error. `None` unless the block lies inside the image.
fn luma_peak(image: &RgbImage, p: Point2<f64>) -> Option<f64> {
    let (x, y) = (p.x.round(), p.y.round());
    if x < 1.0
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

fn luma_bilinear(image: &RgbImage, p: Point2<f64>) -> Option<f64> {
    let (x0, y0) = (p.x.floor(), p.y.floor());
    if x0 < 0.0
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

/// Looks for the pattern's border ring in a color image, given the
/// homography from pattern unit coordinates to undistorted pixels that
/// [`detect`] found.
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
    let aspect = pattern.aspect;
    let undistorted = |x: f64, y: f64| apply_homography(unit_to_pixel, Point2::new(x / aspect, y));
    let raw = |p: Point2<f64>| distort_pixel(intrinsics, p);

    let mut dot_levels: Vec<f64> = pattern
        .dots
        .iter()
        .filter_map(|d| undistorted(d.u * aspect, d.v))
        .filter_map(|p| luma_peak(image, raw(p)))
        .collect();
    let mut black_levels = Vec::new();
    for r in 0..pattern.rows - 1 {
        for c in 0..pattern.cols - 1 {
            let (a, b) = (
                &pattern.dots[pattern.index(c, r)],
                &pattern.dots[pattern.index(c + 1, r + 1)],
            );
            let center = undistorted((a.u + b.u) / 2.0 * aspect, (a.v + b.v) / 2.0);
            if let Some(v) = center.and_then(|p| luma_bilinear(image, raw(p))) {
                black_levels.push(v);
            }
        }
    }
    let (Some(dot), Some(black)) = (median(&mut dot_levels), median(&mut black_levels)) else {
        return EdgeCheck::Unknown;
    };
    let contrast = dot - black;
    if contrast < EDGE_MIN_CONTRAST {
        return EdgeCheck::Unknown;
    }

    let margin = EDGE_RIDGE_FRACTION * contrast;
    let half = FRAME_THICKNESS_H / 2.0;
    let mut cropped = SideSet::default();
    let mut unknown = false;
    for side in 0..4 {
        // Direction from the ring toward the screen's middle.
        let inward = match side {
            0 => (0.0, 1.0),
            1 => (-1.0, 0.0),
            2 => (0.0, -1.0),
            _ => (1.0, 0.0),
        };
        let (mut valid, mut ridges) = (0usize, 0usize);
        for k in 0..EDGE_SAMPLES_PER_SIDE {
            let t = EDGE_SPAN.0
                + (EDGE_SPAN.1 - EDGE_SPAN.0) * k as f64 / (EDGE_SAMPLES_PER_SIDE - 1) as f64;
            let (x, y) = match side {
                0 => (aspect * t, half),
                1 => (aspect - half, t),
                2 => (aspect * t, 1.0 - half),
                _ => (half, t),
            };
            let (Some(on_ring), Some(toward)) = (
                undistorted(x, y),
                undistorted(x + inward.0 * 0.01, y + inward.1 * 0.01),
            ) else {
                continue;
            };
            // How wide the ring appears here: perspective squeezes it on the
            // far side and at steep angles, where it fades below what the
            // camera resolves. Such samples say nothing either way.
            let step = toward - on_ring;
            let ring_px = step.norm() * FRAME_THICKNESS_H / 0.01;
            if ring_px < EDGE_MIN_RING_PX {
                continue;
            }
            let direction = step.normalize();
            let offset = (2.0 * ring_px).clamp(2.5, 8.0);
            let sample = (
                luma_peak(image, raw(on_ring)),
                luma_bilinear(image, raw(on_ring + direction * offset)),
                luma_bilinear(image, raw(on_ring - direction * offset)),
            );
            if let (Some(ring), Some(inner), Some(outer)) = sample {
                valid += 1;
                if ring - inner.max(outer) > margin {
                    ridges += 1;
                }
            }
        }
        if valid < EDGE_MIN_VALID_SAMPLES {
            unknown = true;
        } else if (ridges as f64) < EDGE_VISIBLE_FRACTION * valid as f64 {
            match side {
                0 => cropped.top = true,
                1 => cropped.right = true,
                2 => cropped.bottom = true,
                _ => cropped.left = true,
            }
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

/// Pixel distance between horizontally adjacent dots near the grid center.
fn median_spacing(pattern: &Pattern, h: &Matrix3<f64>) -> Option<f64> {
    let (c, r) = (pattern.cols / 2, pattern.rows / 2);
    let d = |c, r| {
        let dot = &pattern.dots[pattern.index(c, r)];
        apply_homography(h, Point2::new(dot.u, dot.v))
    };
    let center = d(c, r)?;
    let right = (d(c + 1, r)? - center).norm();
    let down = (d(c, r + 1)? - center).norm();
    Some(right.min(down))
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    fn intrinsics() -> Intrinsics {
        Intrinsics {
            width: 1280,
            height: 720,
            fx: 900.0,
            fy: 900.0,
            cx: 640.0,
            cy: 360.0,
            k: [-0.06, 0.01, 0.0, 0.0, 0.0, 0.0],
            p: [0.0004, -0.0002],
        }
    }

    /// Draws the pattern through a known homography with the lens model.
    fn render(pattern: &Pattern, intr: &Intrinsics, h: &Matrix3<f64>) -> RgbImage {
        render_cropped(pattern, intr, h, 0.0)
    }

    /// Like `render`, but the panel shows only the part of the pattern image
    /// that lies `crop` (a fraction of each axis) inside its edges, as an
    /// overscanning display does. Outside the panel the camera sees a lit wall.
    fn render_cropped(
        pattern: &Pattern,
        intr: &Intrinsics,
        h: &Matrix3<f64>,
        crop: f64,
    ) -> RgbImage {
        let mut img = RgbImage::from_pixel(intr.width, intr.height, Rgb([14, 14, 14]));
        let inv = h.try_inverse().unwrap();
        let frame = pattern.frame_rects();
        let panel = crop..=1.0 - crop;
        // Screen region is darker than the surround; fill it by inverse mapping.
        for y in 0..intr.height {
            for x in 0..intr.width {
                let Some(und) = undistort_pixel(intr, Point2::new(x as f64, y as f64)) else {
                    continue;
                };
                let Some(uv) = apply_homography(&inv, und) else {
                    continue;
                };
                let inside = panel.contains(&uv.x) && panel.contains(&uv.y);
                let mut value: f64 = if inside { 5.0 } else { 70.0 };
                if inside {
                    let first = pattern.dots()[0];
                    let (cols, rows) = pattern.grid();
                    let c = (((uv.x - first.u) * pattern.aspect() / GRID_SPACING_H)
                        .round()
                        .max(0.0) as usize)
                        .min(cols - 1);
                    let r = (((uv.y - first.v) / GRID_SPACING_H).round().max(0.0) as usize)
                        .min(rows - 1);
                    let d = pattern.dots()[r * cols + c];
                    let dx = (uv.x - d.u) * pattern.aspect();
                    let dy = uv.y - d.v;
                    let dist = (dx * dx + dy * dy).sqrt();
                    // One screen height is `scale` pixels along v.
                    let scale = (apply_homography(h, Point2::new(uv.x, uv.y + 0.001)).unwrap()
                        - apply_homography(h, uv).unwrap())
                    .norm()
                        / 0.001;
                    let edge = (d.radius_h - dist) * scale + 0.5;
                    if edge > 0.0 {
                        value = value.max(5.0 + 245.0 * edge.min(1.0));
                    }
                    let (px, py) = (uv.x * pattern.aspect(), uv.y);
                    for rect in &frame {
                        let inset = (px - rect.x)
                            .min(rect.x + rect.width - px)
                            .min(py - rect.y)
                            .min(rect.y + rect.height - py);
                        let coverage = (inset * scale + 0.5).clamp(0.0, 1.0);
                        value = value.max(5.0 + 245.0 * coverage);
                    }
                }
                img.put_pixel(x, y, Rgb([value as u8; 3]));
            }
        }
        img
    }

    /// Homography from the unit square to undistorted pixels for a 1.2 m wide
    /// screen at depth `z`, turned about the vertical axis by `yaw` radians
    /// and offset `x` meters sideways.
    fn pose_homography(pattern: &Pattern, yaw: f64, x: f64, z: f64) -> Matrix3<f64> {
        let w = 1.2;
        let h_m = w / pattern.aspect();
        let r = nalgebra::Rotation3::from_euler_angles(0.0, yaw, 0.05);
        let center = nalgebra::Vector3::new(x, 0.0, z);
        let intr = intrinsics();
        let to_px = |u: f64, v: f64| {
            let local = nalgebra::Vector3::new((u - 0.5) * w, (v - 0.5) * h_m, 0.0);
            let p = r * local + center;
            Point2::new(p.x / p.z * intr.fx + intr.cx, p.y / p.z * intr.fy + intr.cy)
        };
        let src = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];
        let dst = src.map(|p| to_px(p.x, p.y));
        fit_homography(&src, &dst).unwrap()
    }

    fn tilted_homography(pattern: &Pattern) -> Matrix3<f64> {
        pose_homography(pattern, 0.35, 0.1, 2.4)
    }

    #[test]
    fn pattern_grid_matches_aspect() {
        let p = Pattern::new(16.0 / 9.0);
        assert_eq!(p.grid(), (16, 9));
        assert_eq!(p.dots().len(), 144);
        let anchors: Vec<_> = p
            .dots()
            .iter()
            .enumerate()
            .filter(|(_, d)| d.anchor)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(anchors, vec![0, 15, 128], "TL, TR, BL");
        assert!(
            p.dots()
                .iter()
                .all(|d| (0.0..=1.0).contains(&d.u) && (0.0..=1.0).contains(&d.v))
        );
        // Round dots: spacing in screen-height units is isotropic.
        let d0 = p.dots()[1];
        let d1 = p.dots()[0];
        assert!(((d0.u - d1.u) * p.aspect() - 0.1).abs() < 1e-9);
        assert!((p.dots()[16].v - p.dots()[0].v - 0.1).abs() < 1e-9);
        assert!(Pattern::new(1.6).grid().0 < 16);
        assert_eq!(Pattern::new(4.0 / 3.0).grid().1, 9);
    }

    #[test]
    fn detects_a_tilted_pattern_and_orders_it_by_the_anchors() {
        let pattern = Pattern::new(16.0 / 9.0);
        let intr = intrinsics();
        let h = tilted_homography(&pattern);
        let image = render(&pattern, &intr, &h);
        let det = detect(&pattern, &image, &intr).expect("pattern found");
        assert_eq!(det.quality.matched, 144, "{:?}", det.quality);
        assert!(det.quality.rms_px < 0.3, "{:?}", det.quality);
        for c in &det.correspondences {
            let truth = apply_homography(&h, c.pattern).unwrap();
            assert!(
                (truth - c.pixel).norm() < 0.4,
                "dot {} off by {}",
                c.index,
                (truth - c.pixel).norm()
            );
        }
        let corner = apply_homography(&h, Point2::new(0.0, 0.0)).unwrap();
        let est = apply_homography(&det.homography, Point2::new(0.0, 0.0)).unwrap();
        assert!((corner - est).norm() < 1.0);
    }

    #[test]
    fn rejects_an_image_without_a_pattern() {
        let intr = intrinsics();
        let image = RgbImage::from_pixel(intr.width, intr.height, Rgb([40, 40, 40]));
        assert!(matches!(
            detect(&Pattern::new(16.0 / 9.0), &image, &intr),
            Err(DetectFailure::NoDots { .. })
        ));
    }

    #[test]
    fn survives_clutter_blobs_and_a_missing_dot() {
        let pattern = Pattern::new(16.0 / 9.0);
        let intr = intrinsics();
        let h = tilted_homography(&pattern);
        let mut image = render(&pattern, &intr, &h);
        // Bright round clutter off the screen and an occluded interior dot.
        for (cx, cy) in [(60i32, 60i32), (1200, 80), (100, 650)] {
            for dy in -5i32..=5 {
                for dx in -5i32..=5 {
                    if dx * dx + dy * dy <= 25 {
                        image.put_pixel((cx + dx) as u32, (cy + dy) as u32, Rgb([255; 3]));
                    }
                }
            }
        }
        let victim =
            apply_homography(&h, Point2::new(pattern.dots()[50].u, pattern.dots()[50].v)).unwrap();
        let victim = distort_pixel(&intr, victim);
        for dy in -12i32..=12 {
            for dx in -12i32..=12 {
                let (x, y) = ((victim.x as i32 + dx) as u32, (victim.y as i32 + dy) as u32);
                image.put_pixel(x, y, Rgb([5; 3]));
            }
        }
        let det = detect(&pattern, &image, &intr).expect("pattern found");
        assert_eq!(det.quality.matched, 143);
        assert!(det.correspondences.iter().all(|c| c.index != 50));
    }

    /// Area centroid of the image of a dot's outline: the polygon through the
    /// projected circle, integrated numerically so it shares no algebra with
    /// [`center_bias`].
    fn outline_centroid(pattern: &Pattern, dot: usize, h: &Matrix3<f64>) -> Point2<f64> {
        let d = pattern.dots()[dot];
        let n = 2000;
        let ring: Vec<Point2<f64>> = (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                let u = (d.u * pattern.aspect() + d.radius_h * a.cos()) / pattern.aspect();
                apply_homography(h, Point2::new(u, d.v + d.radius_h * a.sin())).unwrap()
            })
            .collect();
        let (mut area, mut cx, mut cy) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            let cross = p.x * q.y - q.x * p.y;
            area += cross;
            cx += (p.x + q.x) * cross;
            cy += (p.y + q.y) * cross;
        }
        Point2::new(cx / (3.0 * area), cy / (3.0 * area))
    }

    #[test]
    fn correction_removes_the_centroid_bias_of_noise_free_ellipses() {
        let pattern = Pattern::with_dot_scale(16.0 / 9.0, 1.4);
        let h = pose_homography(&pattern, 1.05, 0.3, 1.3);
        let measured: Vec<(usize, Point2<f64>)> = (0..pattern.dots().len())
            .map(|i| (i, outline_centroid(&pattern, i, &h)))
            .collect();
        let corrected = corrected_centers(&pattern, &h, &measured).unwrap();

        let (mut raw_max, mut fixed_max) = (0.0f64, 0.0f64);
        for ((i, centroid), fixed) in measured.iter().zip(&corrected) {
            let d = pattern.dots()[*i];
            let truth = apply_homography(&h, Point2::new(d.u, d.v)).unwrap();
            raw_max = raw_max.max((centroid - truth).norm());
            fixed_max = fixed_max.max((fixed - truth).norm());
        }
        assert!(
            raw_max > 0.3,
            "the case must have a visible bias, got {raw_max} px"
        );
        assert!(
            fixed_max < raw_max * 1e-4,
            "raw {raw_max} px, corrected {fixed_max} px"
        );
    }

    #[test]
    fn bias_grows_with_dot_size_and_obliqueness_and_points_toward_the_near_side() {
        let pattern = Pattern::new(16.0 / 9.0);
        let big = Pattern::with_dot_scale(16.0 / 9.0, 1.4);
        let norm = |p: &Pattern, h: &Matrix3<f64>| center_bias(p, 60, h).unwrap().norm();
        let frontal = pose_homography(&pattern, 0.0, 0.0, 2.0);
        let steep = pose_homography(&pattern, 1.05, 0.0, 2.0);
        assert!(
            norm(&pattern, &frontal) < 1e-9,
            "a fronto-parallel circle has no bias"
        );
        assert!(
            norm(&pattern, &steep) > norm(&pattern, &pose_homography(&pattern, 0.35, 0.0, 2.0))
        );
        assert!(norm(&big, &steep) > 1.5 * norm(&pattern, &steep));
    }

    #[test]
    fn correction_is_cheap() {
        let pattern = Pattern::new(16.0 / 9.0);
        let h = pose_homography(&pattern, 0.8, 0.1, 2.0);
        let measured: Vec<(usize, Point2<f64>)> = (0..pattern.dots().len())
            .map(|i| {
                (
                    i,
                    apply_homography(&h, Point2::new(pattern.dots()[i].u, pattern.dots()[i].v))
                        .unwrap(),
                )
            })
            .collect();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(corrected_centers(&pattern, &h, &measured).unwrap());
        }
        let per_call = start.elapsed() / 100;
        eprintln!(
            "corrected_centers for {} dots: {per_call:?}",
            measured.len()
        );
        assert!(
            per_call < std::time::Duration::from_millis(5),
            "{per_call:?}"
        );
    }

    /// Mean distance of the detected dot centers from the true image of the
    /// dot centers, for the corrected estimate and the raw centroid.
    fn detection_errors(pattern: &Pattern, h: &Matrix3<f64>) -> (f64, f64, f64) {
        let intr = intrinsics();
        let image = render(pattern, &intr, h);
        let det = detect(pattern, &image, &intr).expect("pattern found");
        let (mut fixed, mut raw, mut n) = (0.0, 0.0, 0.0);
        for c in &det.correspondences {
            let truth = apply_homography(h, c.pattern).unwrap();
            fixed += (c.pixel - truth).norm();
            raw += (c.centroid - truth).norm();
            n += 1.0;
        }
        (fixed / n, raw / n, n)
    }

    #[test]
    fn corrected_centers_beat_centroids_on_a_rendered_steep_pattern() {
        let pattern = Pattern::with_dot_scale(16.0 / 9.0, 1.4);
        let h = pose_homography(&pattern, 1.05, 0.3, 1.3);
        let (fixed, raw, n) = detection_errors(&pattern, &h);
        eprintln!(
            "steep, large dots: {n} dots, mean error {raw:.3} px raw centroid, {fixed:.3} px corrected"
        );
        assert!(n >= 100.0);
        assert!(fixed < 0.5 * raw, "corrected {fixed} px vs raw {raw} px");
    }

    #[test]
    fn frame_is_a_ring_and_brackets_that_stay_clear_of_the_dots() {
        let pattern = Pattern::new(16.0 / 9.0);
        let rects = pattern.frame_rects();
        assert_eq!(rects.len(), 12);
        for r in &rects {
            assert!(
                r.x >= 0.0
                    && r.y >= 0.0
                    && r.x + r.width <= pattern.aspect() + 1e-12
                    && r.y + r.height <= 1.0 + 1e-12,
                "{r:?}"
            );
        }
        // No dot touches a rectangle, so the ring cannot merge with a dot blob.
        for d in pattern.dots() {
            let (cx, cy) = (d.u * pattern.aspect(), d.v);
            for r in &rects {
                let nearest = ((cx.clamp(r.x, r.x + r.width) - cx).powi(2)
                    + (cy.clamp(r.y, r.y + r.height) - cy).powi(2))
                .sqrt();
                assert!(
                    nearest > d.radius_h + 0.01,
                    "dot at ({cx}, {cy}) is {nearest} from {r:?}"
                );
            }
        }
    }

    #[test]
    fn ring_does_not_disturb_detection_and_reads_as_visible() {
        let pattern = Pattern::new(16.0 / 9.0);
        let intr = intrinsics();
        for (h, steep) in [
            (tilted_homography(&pattern), false),
            (pose_homography(&pattern, 0.9, 0.2, 1.6), true),
        ] {
            let image = render(&pattern, &intr, &h);
            let det = detect(&pattern, &image, &intr).expect("pattern found");
            assert_eq!(det.quality.matched, 144, "{:?}", det.quality);
            assert!(det.quality.rms_px < 0.3, "{:?}", det.quality);
            let edges = check_edges(&pattern, &image, &intr, &det.homography);
            if steep {
                // The far border is too narrow to judge, which must not read as cropped.
                assert!(!matches!(edges, EdgeCheck::Cropped { .. }), "{edges:?}");
            } else {
                assert_eq!(edges, EdgeCheck::Visible);
            }
        }
    }

    #[test]
    fn cropped_image_is_reported_side_by_side() {
        let pattern = Pattern::new(16.0 / 9.0);
        let intr = intrinsics();
        let h = tilted_homography(&pattern);
        // 1.5% off every edge: the ring (0.6% of the height) and brackets are gone on all four sides.
        let image = render_cropped(&pattern, &intr, &h, 0.015);
        let det = detect(&pattern, &image, &intr).expect("pattern found");
        assert_eq!(
            check_edges(&pattern, &image, &intr, &det.homography),
            EdgeCheck::Cropped {
                sides: SideSet {
                    top: true,
                    right: true,
                    bottom: true,
                    left: true
                }
            }
        );
    }

    #[test]
    fn edges_are_unknown_when_nothing_can_be_judged() {
        let pattern = Pattern::new(16.0 / 9.0);
        let intr = intrinsics();
        let h = tilted_homography(&pattern);
        // The camera sees only the middle of a pattern drawn three times as large,
        // so every ring location is outside the view.
        let zoom = Matrix3::new(3.0, 0.0, -1.0, 0.0, 3.0, -1.0, 0.0, 0.0, 1.0);
        let zoomed_h = h * zoom;
        let zoomed = render(&pattern, &intr, &zoomed_h);
        assert_eq!(
            check_edges(&pattern, &zoomed, &intr, &zoomed_h),
            EdgeCheck::Unknown
        );
        // A washed-out picture has no contrast to judge by.
        let faint = RgbImage::from_pixel(intr.width, intr.height, Rgb([30, 30, 30]));
        assert_eq!(check_edges(&pattern, &faint, &intr, &h), EdgeCheck::Unknown);
    }

    /// A room of isolated bright spots, some bigger than the corner dots, must not
    /// hide a pattern that is plainly in view: the corner dots are the ones with
    /// dots beside them, not the ones that merely stand out.
    #[test]
    fn bright_clutter_around_the_screen_does_not_hide_the_pattern() {
        let pattern = Pattern::new(16.0 / 9.0);
        let intr = intrinsics();
        let h = pose_homography(&pattern, 0.15, 0.0, 2.2);
        let mut image = render(&pattern, &intr, &h);

        // The screen's bounding box, so the clutter stays off the screen.
        let (mut lo, mut hi) = (
            Point2::new(f64::MAX, f64::MAX),
            Point2::new(f64::MIN, f64::MIN),
        );
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let p = distort_pixel(&intr, apply_homography(&h, Point2::new(u, v)).unwrap());
            lo = Point2::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point2::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let margin = 25.0;

        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut placed = 0;
        while placed < 120 {
            let (cx, cy) = (next() * 1280.0, next() * 720.0);
            let radius = 3.0 + next() * 9.0;
            let near_screen = cx > lo.x - margin - radius
                && cx < hi.x + margin + radius
                && cy > lo.y - margin - radius
                && cy < hi.y + margin + radius;
            if near_screen {
                continue;
            }
            for y in (cy - radius).floor() as i32..=(cy + radius).ceil() as i32 {
                for x in (cx - radius).floor() as i32..=(cx + radius).ceil() as i32 {
                    let inside = (x as f64 - cx).hypot(y as f64 - cy) <= radius;
                    if inside && (0..1280).contains(&x) && (0..720).contains(&y) {
                        image.put_pixel(x as u32, y as u32, Rgb([235; 3]));
                    }
                }
            }
            placed += 1;
        }

        let found = detect(&pattern, &image, &intr).expect("the pattern is in view");
        assert!(
            found.correspondences.len() * 10 >= pattern.dots().len() * 9,
            "{} of {} dots",
            found.correspondences.len(),
            pattern.dots().len()
        );
    }
}
