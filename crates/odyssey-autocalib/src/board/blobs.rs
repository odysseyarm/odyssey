//! Dot candidates in a color image.
//!
//! Stages: gray image, local white level, dark mask, connected components,
//! shape test, then sub-pixel measurement of each surviving blob against the
//! raw image.

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::ops::RangeInclusive;

use image::RgbImage;
use nalgebra::{Matrix3, Point2, Vector3};

use super::median;
use crate::camera::Intrinsics;
use crate::plane::undistort_pixel;

/// Images with a side shorter than this cannot hold a board that is
/// detectable at all.
const MIN_IMAGE_SIDE_PX: usize = 16;

/// Smallest accepted blob, in pixels (a dot about four pixels across).
const MIN_BLOB_AREA_PX: usize = 12;
/// Blobs keep at most this fraction of the image.
const MAX_BLOB_AREA_FRACTION: f64 = 0.03;
/// A blob whose bounding box comes closer than this to the image border is
/// cut off by it, so its shape and center cannot be trusted.
const BORDER_MARGIN_PX: usize = 2;
/// A blob's area over the area of the ellipse with the same second moments
/// must fall in this range for the blob to count as round.
const ELLIPSE_FILL_RANGE: RangeInclusive<f64> = 0.75..=1.25;
/// Shortest over longest axis of a blob's ellipse. Thinner blobs are lines.
const MIN_AXIS_RATIO: f64 = 0.3;
/// Variance along each axis that one pixel's own area adds to a blob's
/// second moments.
const PIXEL_VARIANCE: f64 = 1.0 / 12.0;

/// A pixel is dark when it is under this fraction of the local white level.
const DARK_FRACTION: f32 = 0.6;
/// ...and at least this many gray levels under it.
const MIN_DARK_DROP: f32 = 25.0;
/// Local white level under this (0..255) is background, not paper.
const MIN_WHITE_LEVEL: f32 = 60.0;

/// The white level is computed on a grid with about this many cells along the
/// image's short side.
const WHITE_GRID_CELLS_ALONG_SHORT_SIDE: usize = 160;
/// The closing window reaches this fraction (as a divisor) of the short side
/// to each side of a pixel: it then holds dots up to a fifth of the short
/// side across.
const WHITE_WINDOW_RADIUS_DIVISOR: usize = 10;
/// ...but never fewer than this many grid cells.
const MIN_WHITE_WINDOW_RADIUS_CELLS: usize = 2;

/// A blob is measured against the paper around it. The ring of paper samples
/// starts this many pixels outside the blob's bounding box. The darkness
/// centroid reaches the same distance beyond the blob, so antialiased edges
/// count in proportion to their darkness.
const EDGE_ALLOWANCE_PX: usize = 2;
/// Width of the paper ring: a quarter of the blob's size, but at least this
/// many pixels.
const MIN_RING_WIDTH_PX: usize = 4;
/// Fewest paper pixels in the ring for the white plane to be fitted.
const MIN_RING_SAMPLES: usize = 12;
/// The white plane is refitted without ring pixels farther than this many
/// standard deviations from it (printed marks and shadows next to the dot).
const RING_OUTLIER_SIGMAS: f64 = 2.5;
/// Floor on the standard deviation in that rejection, gray levels. Without it
/// a noiseless ring rejects its own pixels.
const MIN_RING_SIGMA: f64 = 1.0;
/// The paper around a dot is clean when the white plane's residual standard
/// deviation is at most this fraction of the white level.
const MAX_WHITE_NOISE_FRACTION: f64 = 0.15;
/// The darkest `1/DARKEST_SHARE_DENOMINATOR` of a blob's pixels stand for the
/// printed black.
const DARKEST_SHARE_DENOMINATOR: usize = 5;
/// A dot must lose at least this fraction of the paper's brightness.
const MIN_DOT_CONTRAST: f64 = 0.3;

/// Most candidate blobs the grid search looks at.
const MAX_BLOBS: usize = 120;
/// The size filter needs at least this many blobs to have a typical size.
const MIN_BLOBS_FOR_SIZE_FILTER: usize = 4;
/// Blobs whose diameter over the median diameter falls outside this range
/// are letters, ink specks or large dark patches: dots of one board look
/// alike, and perspective changes their size across the board by tens of
/// percent, not by a factor of 2.5.
const TYPICAL_DIAMETER_RATIO: RangeInclusive<f64> = 0.4..=2.5;

/// A dot candidate that passed the shape, contrast and size tests.
#[derive(Debug, Clone, Copy)]
pub(super) struct DotBlob {
    /// Undistorted (pinhole) centroid of the blob's darkness.
    pub(super) center: Point2<f64>,
    pub(super) diameter_px: f64,
}

/// The dot candidates of a color image, at most [`MAX_BLOBS`].
pub(super) fn find_dots(color: &RgbImage, intrinsics: &Intrinsics) -> Vec<DotBlob> {
    let (w, h) = (color.width() as usize, color.height() as usize);
    if w < MIN_IMAGE_SIDE_PX || h < MIN_IMAGE_SIDE_PX {
        return Vec::new();
    }
    let raw = luma(color);
    let smooth = box_blur3(&raw, w, h);
    let white = WhiteLevel::new(&smooth, w, h);
    let mask = dark_mask(&smooth, &white, w, h);

    let mut blobs = Vec::new();
    for component in components(&mask, w, h) {
        if !plausible_dot_shape(&component, w, h) {
            continue;
        }
        let Some((center, diameter_px)) = measure_blob(&raw, &mask, w, h, &component) else {
            continue;
        };
        let Some(center) = undistort_pixel(intrinsics, center) else {
            continue;
        };
        blobs.push(DotBlob {
            center,
            diameter_px,
        });
    }
    drop_size_outliers(&mut blobs);
    blobs
}

fn luma(image: &RgbImage) -> Vec<f32> {
    image
        .pixels()
        .map(|p| 0.299 * f32::from(p.0[0]) + 0.587 * f32::from(p.0[1]) + 0.114 * f32::from(p.0[2]))
        .collect()
}

fn box_blur3(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; data.len()];
    for y in 0..h {
        let row = &data[y * w..(y + 1) * w];
        for x in 0..w {
            let (a, b) = (x.saturating_sub(1), (x + 1).min(w - 1));
            tmp[y * w + x] = (row[a] + row[x] + row[b]) / 3.0;
        }
    }
    let mut out = vec![0.0f32; data.len()];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (tmp[a * w + x] + tmp[y * w + x] + tmp[b * w + x]) / 3.0;
        }
    }
    out
}

/// Sliding-window maximum or minimum over `[i - radius, i + radius]`, clipped
/// to the line. Linear time with a monotonic deque.
fn slide_extreme(
    src: &[f32],
    radius: usize,
    max: bool,
    dst: &mut [f32],
    queue: &mut VecDeque<usize>,
) {
    let n = src.len();
    queue.clear();
    let better = |a: f32, b: f32| if max { a >= b } else { a <= b };
    let mut next = 0;
    for (i, slot) in dst.iter_mut().enumerate().take(n) {
        let hi = (i + radius).min(n - 1);
        while next <= hi {
            while queue.back().is_some_and(|&j| better(src[next], src[j])) {
                queue.pop_back();
            }
            queue.push_back(next);
            next += 1;
        }
        while queue.front().is_some_and(|&j| j + radius < i) {
            queue.pop_front();
        }
        *slot = src[*queue.front().expect("the window always holds index i")];
    }
}

/// Replaces every pixel by the extreme over the square window of `radius`
/// around it (rows first, then columns).
fn window_extreme(data: &mut [f32], w: usize, h: usize, radius: usize, max: bool) {
    let mut queue = VecDeque::new();
    let mut out = vec![0.0f32; w.max(h)];
    for row in data.chunks_exact_mut(w) {
        slide_extreme(row, radius, max, &mut out[..w], &mut queue);
        row.copy_from_slice(&out[..w]);
    }
    let mut column = vec![0.0f32; h];
    for x in 0..w {
        for (slot, &value) in column.iter_mut().zip(data[x..].iter().step_by(w)) {
            *slot = value;
        }
        slide_extreme(&column, radius, max, &mut out[..h], &mut queue);
        for (slot, &value) in data[x..].iter_mut().step_by(w).zip(&out[..h]) {
            *slot = value;
        }
    }
}

/// Local white level: a morphological closing of the smoothed image, computed
/// on a reduced grid (block maximum, then dilate and erode). The closing fills
/// every dark feature smaller than its window, so dots vanish from it while
/// uneven lighting and glare survive. Dark surroundings larger than the
/// window stay dark, so the reference does not get dragged up there.
#[derive(Debug)]
struct WhiteLevel {
    data: Vec<f32>,
    width: usize,
    scale: usize,
}

impl WhiteLevel {
    fn new(smooth: &[f32], w: usize, h: usize) -> Self {
        let scale = (w.min(h) / WHITE_GRID_CELLS_ALONG_SHORT_SIDE).max(1);
        let (cw, ch) = (w.div_ceil(scale), h.div_ceil(scale));
        let mut data = vec![0.0f32; cw * ch];
        for y in 0..h {
            for x in 0..w {
                let cell = &mut data[(y / scale) * cw + x / scale];
                *cell = cell.max(smooth[y * w + x]);
            }
        }
        let radius =
            (w.min(h) / WHITE_WINDOW_RADIUS_DIVISOR / scale).max(MIN_WHITE_WINDOW_RADIUS_CELLS);
        window_extreme(&mut data, cw, ch, radius, true);
        window_extreme(&mut data, cw, ch, radius, false);
        Self {
            data,
            width: cw,
            scale,
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.data[(y / self.scale) * self.width + x / self.scale]
    }
}

/// Pixels clearly darker than the paper around them.
fn dark_mask(smooth: &[f32], white: &WhiteLevel, w: usize, h: usize) -> Vec<bool> {
    let mut mask = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let reference = white.at(x, y);
            let v = smooth[y * w + x];
            mask[y * w + x] = reference > MIN_WHITE_LEVEL
                && v < DARK_FRACTION * reference
                && reference - v > MIN_DARK_DROP;
        }
    }
    mask
}

/// Inclusive pixel rectangle.
#[derive(Debug, Clone, Copy)]
struct PixelBox {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl PixelBox {
    fn width(&self) -> usize {
        self.x1 - self.x0 + 1
    }

    fn height(&self) -> usize {
        self.y1 - self.y0 + 1
    }

    fn center(&self) -> (f64, f64) {
        (
            (self.x0 + self.x1) as f64 / 2.0,
            (self.y0 + self.y1) as f64 / 2.0,
        )
    }

    fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Grown by `margin` pixels on every side, clipped to a `w` x `h` image.
    fn grown(&self, margin: usize, w: usize, h: usize) -> Self {
        Self {
            x0: self.x0.saturating_sub(margin),
            y0: self.y0.saturating_sub(margin),
            x1: (self.x1 + margin).min(w - 1),
            y1: (self.y1 + margin).min(h - 1),
        }
    }
}

#[derive(Debug)]
struct Component {
    /// Pixel indices `y * width + x`.
    pixels: Vec<u32>,
    bounds: PixelBox,
}

impl Component {
    fn coordinates(&self, w: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.pixels
            .iter()
            .map(move |&p| (p as usize % w, p as usize / w))
    }
}

/// Eight-connected components of the mask.
fn components(mask: &[bool], w: usize, h: usize) -> Vec<Component> {
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start as u32);
        let mut comp = Component {
            pixels: Vec::new(),
            bounds: PixelBox {
                x0: usize::MAX,
                y0: usize::MAX,
                x1: 0,
                y1: 0,
            },
        };
        while let Some(p) = stack.pop() {
            let (x, y) = (p as usize % w, p as usize / w);
            comp.pixels.push(p);
            comp.bounds.x0 = comp.bounds.x0.min(x);
            comp.bounds.y0 = comp.bounds.y0.min(y);
            comp.bounds.x1 = comp.bounds.x1.max(x);
            comp.bounds.y1 = comp.bounds.y1.max(y);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let q = ny * w + nx;
                    if mask[q] && !seen[q] {
                        seen[q] = true;
                        stack.push(q as u32);
                    }
                }
            }
        }
        out.push(comp);
    }
    out
}

/// Shape test on a thresholded blob: size, border contact, ellipse-likeness
/// (the area of an ellipse with the blob's second moments must match the blob)
/// and axis ratio.
#[expect(clippy::manual_midpoint, reason = "preserve covariance mean rounding")]
fn plausible_dot_shape(c: &Component, w: usize, h: usize) -> bool {
    let area = c.pixels.len();
    if area < MIN_BLOB_AREA_PX || area as f64 > MAX_BLOB_AREA_FRACTION * (w * h) as f64 {
        return false;
    }
    let b = &c.bounds;
    if b.x0 < BORDER_MARGIN_PX
        || b.y0 < BORDER_MARGIN_PX
        || b.x1 + BORDER_MARGIN_PX >= w
        || b.y1 + BORDER_MARGIN_PX >= h
    {
        return false;
    }
    let n = area as f64;
    let (mut sx, mut sy) = (0.0, 0.0);
    for (x, y) in c.coordinates(w) {
        sx += x as f64;
        sy += y as f64;
    }
    let (mx, my) = (sx / n, sy / n);
    let (mut cxx, mut cxy, mut cyy) = (0.0, 0.0, 0.0);
    for (x, y) in c.coordinates(w) {
        let dx = x as f64 - mx;
        let dy = y as f64 - my;
        cxx += dx * dx;
        cxy += dx * dy;
        cyy += dy * dy;
    }
    let (cxx, cxy, cyy) = (cxx / n + PIXEL_VARIANCE, cxy / n, cyy / n + PIXEL_VARIANCE);
    let half_trace = (cxx + cyy) / 2.0;
    let disc = (((cxx - cyy) / 2.0).powi(2) + cxy * cxy).sqrt();
    let (l1, l2) = (half_trace + disc, half_trace - disc);
    if l2 <= 0.0 {
        return false;
    }
    let fill = n / (PI * 4.0 * (l1 * l2).sqrt());
    ELLIPSE_FILL_RANGE.contains(&fill) && (l2 / l1).sqrt() >= MIN_AXIS_RATIO
}

/// The paper's brightness around a dot, as the plane `I = a + b dx + c dy`
/// fitted to the pixels of a ring around it.
#[derive(Debug)]
struct WhitePlane {
    /// `a`, `b`, `c`.
    coefficients: [f64; 3],
    /// Pixel position that `dx` and `dy` count from.
    origin: (f64, f64),
    /// Residual standard deviation of the ring pixels.
    sigma: f64,
}

impl WhitePlane {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.coefficients[0]
            + self.coefficients[1] * (x as f64 - self.origin.0)
            + self.coefficients[2] * (y as f64 - self.origin.1)
    }
}

/// Least-squares plane `I = a + b dx + c dy` through the pixels around a dot,
/// with one round of outlier rejection. Returns the coefficients and the
/// residual standard deviation.
fn fit_white_plane(samples: &[(f64, f64, f64)]) -> Option<([f64; 3], f64)> {
    let mut active: Vec<usize> = (0..samples.len()).collect();
    let mut result = None;
    for round in 0..2 {
        if active.len() < MIN_RING_SAMPLES {
            return None;
        }
        let mut m = Matrix3::zeros();
        let mut b = Vector3::zeros();
        for &i in &active {
            let (dx, dy, v) = samples[i];
            let row = Vector3::new(1.0, dx, dy);
            m += row * row.transpose();
            b += row * v;
        }
        let coef = m.try_inverse()? * b;
        let residual = |s: &(f64, f64, f64)| s.2 - (coef.x + coef.y * s.0 + coef.z * s.1);
        let sigma = (active
            .iter()
            .map(|&i| residual(&samples[i]).powi(2))
            .sum::<f64>()
            / active.len() as f64)
            .sqrt();
        result = Some(([coef.x, coef.y, coef.z], sigma));
        if round == 0 {
            let keep = RING_OUTLIER_SIGMAS * sigma.max(MIN_RING_SIGMA);
            active.retain(|&i| residual(&samples[i]).abs() <= keep);
        }
    }
    result
}

/// The white level around a blob: fitted to the pixels of a ring just outside
/// `inner` that no dark blob covers.
fn fit_ring_white(
    raw: &[f32],
    mask: &[bool],
    w: usize,
    h: usize,
    c: &Component,
    inner: &PixelBox,
) -> Option<WhitePlane> {
    let size = c.bounds.width().max(c.bounds.height());
    let ring_width = MIN_RING_WIDTH_PX.max(size / 4);
    let outer = c.bounds.grown(EDGE_ALLOWANCE_PX + ring_width, w, h);
    let origin = c.bounds.center();
    let mut samples = Vec::new();
    for y in outer.y0..=outer.y1 {
        for x in outer.x0..=outer.x1 {
            if !inner.contains(x, y) && !mask[y * w + x] {
                samples.push((
                    x as f64 - origin.0,
                    y as f64 - origin.1,
                    f64::from(raw[y * w + x]),
                ));
            }
        }
    }
    let (coefficients, sigma) = fit_white_plane(&samples)?;
    Some(WhitePlane {
        coefficients,
        origin,
        sigma,
    })
}

/// `mask` plus every pixel next to a set pixel (eight neighbors).
fn dilated(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut out = mask.to_vec();
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                continue;
            }
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
            out[y * w + x] = (y0..=y1).any(|yy| (x0..=x1).any(|xx| mask[yy * w + xx]));
        }
    }
    out
}

/// Intensity-weighted centroid of (white - pixel) over the blob grown by
/// [`EDGE_ALLOWANCE_PX`] (`inner`), so antialiased edges count in proportion
/// to their darkness. Pixels that belong to some other dark blob are left
/// out, so a mark just outside the dot (the scale bar, a letter) cannot pull
/// the center. Returns the centroid and the total weight, or `None` when
/// there is no darkness at all.
fn darkness_centroid(
    raw: &[f32],
    mask: &[bool],
    w: usize,
    c: &Component,
    inner: &PixelBox,
    white: &WhitePlane,
) -> Option<(f64, f64, f64)> {
    let (bw, bh) = (inner.width(), inner.height());
    let mut own = vec![false; bw * bh];
    for (x, y) in c.coordinates(w) {
        own[(y - inner.y0) * bw + (x - inner.x0)] = true;
    }
    let mut near = own.clone();
    for _ in 0..EDGE_ALLOWANCE_PX {
        near = dilated(&near, bw, bh);
    }
    let (mut sw, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in inner.y0..=inner.y1 {
        for x in inner.x0..=inner.x1 {
            let local = (y - inner.y0) * bw + (x - inner.x0);
            if !near[local] || (mask[y * w + x] && !own[local]) {
                continue;
            }
            let weight = white.at(x, y) - f64::from(raw[y * w + x]);
            sw += weight;
            sx += weight * x as f64;
            sy += weight * y as f64;
        }
    }
    if sw <= 0.0 {
        return None;
    }
    Some((sx / sw, sy / sw, sw))
}

/// Measures one thresholded blob against the raw image: intensity-weighted
/// centroid of the darkness relative to a locally fitted white level, and the
/// equivalent diameter. `None` when the blob's surroundings are not clean
/// paper or the contrast is too low.
fn measure_blob(
    raw: &[f32],
    mask: &[bool],
    w: usize,
    h: usize,
    c: &Component,
) -> Option<(Point2<f64>, f64)> {
    let inner = c.bounds.grown(EDGE_ALLOWANCE_PX, w, h);
    let white_plane = fit_ring_white(raw, mask, w, h, c, &inner)?;
    let (cx, cy, darkness) = darkness_centroid(raw, mask, w, c, &inner, &white_plane)?;
    let white = white_plane.at(cx.round() as usize, cy.round() as usize);
    if white < f64::from(MIN_WHITE_LEVEL) || white_plane.sigma > MAX_WHITE_NOISE_FRACTION * white {
        return None;
    }
    let mut levels: Vec<f32> = c.pixels.iter().map(|&p| raw[p as usize]).collect();
    levels.sort_by(f32::total_cmp);
    let darkest = (levels.len() / DARKEST_SHARE_DENOMINATOR).max(1);
    let black = levels[..darkest].iter().map(|&v| f64::from(v)).sum::<f64>() / darkest as f64;
    if (white - black) / white < MIN_DOT_CONTRAST {
        return None;
    }
    let area = darkness / (white - black);
    Some((Point2::new(cx, cy), 2.0 * (area / PI).sqrt()))
}

/// Drops blobs far from the typical dot size and keeps the [`MAX_BLOBS`]
/// closest to it.
fn drop_size_outliers(blobs: &mut Vec<DotBlob>) {
    if blobs.len() < MIN_BLOBS_FOR_SIZE_FILTER {
        return;
    }
    let typical = median(&mut blobs.iter().map(|b| b.diameter_px).collect::<Vec<_>>());
    blobs.retain(|b| TYPICAL_DIAMETER_RATIO.contains(&(b.diameter_px / typical)));
    if blobs.len() > MAX_BLOBS {
        blobs.sort_by(|a, b| {
            (a.diameter_px / typical)
                .ln()
                .abs()
                .total_cmp(&(b.diameter_px / typical).ln().abs())
        });
        blobs.truncate(MAX_BLOBS);
    }
}
