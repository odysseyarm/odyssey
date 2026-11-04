//! Dot-shaped bright blobs in a color image: connected components of the pixels that stand
//! out from their neighborhood, filtered by shape.

use image::RgbImage;
use nalgebra::Point2;

use super::LUMA_WEIGHTS;
use crate::camera::Intrinsics;
use crate::plane::undistort_pixel;

/// A blob is foreground when it exceeds its neighborhood's mean by this share of the image's
/// brightness range (see [`BRIGHT_PERCENTILE`] and [`DARK_PERCENTILE`]).
const MARGIN_RANGE_SHARE: f64 = 0.25;
/// The smallest foreground margin, in 8-bit levels, for images with almost no contrast.
const MIN_MARGIN: f64 = 12.0;
/// Brightness percentile taken as the top of the image's range.
const BRIGHT_PERCENTILE: f64 = 0.995;
/// Brightness percentile taken as the bottom of the image's range.
const DARK_PERCENTILE: f64 = 0.05;
/// The neighborhood window's half-size is the image height divided by this ...
const WINDOW_HEIGHT_DIVISOR: usize = 24;
/// ... but at least this many pixels.
const MIN_WINDOW_HALF: usize = 7;
/// A dot covers at most the area of a square this share of the image height on a side.
const MAX_SIDE_HEIGHT_SHARE: f64 = 0.08;
/// The fewest pixels a blob may have.
const MIN_AREA_PX: u32 = 5;
/// Blobs whose bounding box is narrower than this are too small to judge the fill ratio.
const TINY_SIDE_PX: f64 = 4.0;
/// Allowed width-to-height ratio of a blob's bounding box.
const BBOX_ASPECT: std::ops::RangeInclusive<f64> = 0.4..=2.5;
/// The smallest minor-to-major axis ratio of a blob's pixel distribution.
const MIN_AXIS_RATIO: f64 = 0.25;
/// Allowed share of the bounding box a blob fills; a disc fills pi/4, about 0.79.
const FILL_RATIO: std::ops::RangeInclusive<f64> = 0.5..=0.95;

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

/// Running sums over the pixels of one region, from which its [`Component`] follows.
struct RegionSums {
    area: u32,
    weight: f64,
    weighted_x: f64,
    weighted_y: f64,
    x: f64,
    y: f64,
    xx: f64,
    xy: f64,
    yy: f64,
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
}

impl RegionSums {
    fn new(x: usize, y: usize) -> Self {
        Self {
            area: 0,
            weight: 0.0,
            weighted_x: 0.0,
            weighted_y: 0.0,
            x: 0.0,
            y: 0.0,
            xx: 0.0,
            xy: 0.0,
            yy: 0.0,
            min_x: x,
            max_x: x,
            min_y: y,
            max_y: y,
        }
    }

    fn add(&mut self, x: usize, y: usize, weight: f64) {
        self.area += 1;
        self.weight += weight;
        self.weighted_x += weight * x as f64;
        self.weighted_y += weight * y as f64;
        self.x += x as f64;
        self.y += y as f64;
        self.xx += (x * x) as f64;
        self.xy += (x * y) as f64;
        self.yy += (y * y) as f64;
        self.min_x = self.min_x.min(x);
        self.max_x = self.max_x.max(x);
        self.min_y = self.min_y.min(y);
        self.max_y = self.max_y.max(y);
    }

    fn finish(&self, width: usize, height: usize) -> Component {
        let n = f64::from(self.area);
        let (cx, cy) = (self.x / n, self.y / n);
        let (vxx, vxy, vyy) = (
            self.xx / n - cx * cx,
            self.xy / n - cx * cy,
            self.yy / n - cy * cy,
        );
        let half_trace = (vxx + vyy) / 2.0;
        let discriminant = (((vxx - vyy) / 2.0).powi(2) + vxy * vxy).sqrt();
        // The floor keeps a one-pixel-wide line from dividing by zero.
        let (major, minor) = (
            (half_trace + discriminant).max(1e-12),
            (half_trace - discriminant).max(0.0),
        );
        Component {
            area: self.area,
            center: Point2::new(self.weighted_x / self.weight, self.weighted_y / self.weight),
            bbox_w: (self.max_x - self.min_x + 1) as u32,
            bbox_h: (self.max_y - self.min_y + 1) as u32,
            touches_border: self.min_x == 0
                || self.min_y == 0
                || self.max_x + 1 == width
                || self.max_y + 1 == height,
            axis_ratio: (minor / major).sqrt(),
        }
    }
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
    for start_y in 0..h {
        for start_x in 0..w {
            if seen[start_y * w + start_x] {
                continue;
            }
            seen[start_y * w + start_x] = true;
            if weight(start_x as u32, start_y as u32) <= 0.0 {
                continue;
            }
            stack.push((start_x, start_y));
            let mut sums = RegionSums::new(start_x, start_y);
            while let Some((x, y)) = stack.pop() {
                sums.add(x, y, weight(x as u32, y as u32));
                let neighbors = [
                    (x > 0).then(|| (x - 1, y)),
                    (x + 1 < w).then(|| (x + 1, y)),
                    (y > 0).then(|| (x, y - 1)),
                    (y + 1 < h).then(|| (x, y + 1)),
                ];
                for (nx, ny) in neighbors.into_iter().flatten() {
                    if seen[ny * w + nx] {
                        continue;
                    }
                    seen[ny * w + nx] = true;
                    if weight(nx as u32, ny as u32) > 0.0 {
                        stack.push((nx, ny));
                    }
                }
            }
            out.push(sums.finish(w, h));
        }
    }
    out
}

fn luma(image: &RgbImage) -> Vec<u8> {
    let [r, g, b] = LUMA_WEIGHTS;
    image
        .pixels()
        .map(|p| ((r * u32::from(p[0]) + g * u32::from(p[1]) + b * u32::from(p[2])) >> 8) as u8)
        .collect()
}

/// Integral image for windowed means.
struct Integral {
    stride: usize,
    sums: Vec<u64>,
}

impl Integral {
    fn new(gray: &[u8], w: usize, h: usize) -> Self {
        let stride = w + 1;
        let mut sums = vec![0u64; stride * (h + 1)];
        for y in 0..h {
            let mut row = 0u64;
            for x in 0..w {
                row += u64::from(gray[y * w + x]);
                sums[(y + 1) * stride + x + 1] = sums[y * stride + x + 1] + row;
            }
        }
        Self { stride, sums }
    }

    /// Mean over the square window of half-size `half` around `(x, y)`, clipped to the image.
    fn mean(&self, x: usize, y: usize, half: usize, width: usize, height: usize) -> f64 {
        let (x0, x1) = (x.saturating_sub(half), (x + half + 1).min(width));
        let (y0, y1) = (y.saturating_sub(half), (y + half + 1).min(height));
        let s = self.sums[y1 * self.stride + x1] + self.sums[y0 * self.stride + x0]
            - self.sums[y0 * self.stride + x1]
            - self.sums[y1 * self.stride + x0];
        s as f64 / ((x1 - x0) * (y1 - y0)) as f64
    }
}

/// A dot-shaped blob in the raw image.
#[derive(Debug, Clone, Copy)]
pub(super) struct Blob {
    pub raw: Point2<f64>,
    pub undistorted: Point2<f64>,
    pub area: f64,
}

/// The brightness below which the share `p` of the pixels lie, from their histogram.
fn percentile(histogram: &[u32; 256], pixel_count: usize, p: f64) -> f64 {
    let target = (p * pixel_count as f64) as u32;
    let mut accumulated = 0;
    for (value, &count) in histogram.iter().enumerate() {
        accumulated += count;
        if accumulated >= target {
            return value as f64;
        }
    }
    255.0
}

/// How far above its neighborhood's mean a pixel must be to count as foreground: a share of
/// the image's own brightness range, so it tracks screen brightness and room lighting.
fn foreground_margin(gray: &[u8]) -> f64 {
    let mut histogram = [0u32; 256];
    for &g in gray {
        histogram[g as usize] += 1;
    }
    let range = percentile(&histogram, gray.len(), BRIGHT_PERCENTILE)
        - percentile(&histogram, gray.len(), DARK_PERCENTILE);
    (MARGIN_RANGE_SHARE * range).max(MIN_MARGIN)
}

/// Whether `c` looks like the image of a dot: inside the frame, of plausible size, and
/// roughly round and filled.
fn is_dot_shaped(c: &Component, max_area: f64) -> bool {
    let a = f64::from(c.area);
    let (bw, bh) = (f64::from(c.bbox_w), f64::from(c.bbox_h));
    let tiny = bw.min(bh) < TINY_SIDE_PX;
    let fill = a / (bw * bh);
    !c.touches_border
        && a >= f64::from(MIN_AREA_PX)
        && a <= max_area
        && BBOX_ASPECT.contains(&(bw / bh))
        && c.axis_ratio >= MIN_AXIS_RATIO
        && (tiny || FILL_RATIO.contains(&fill))
}

/// Finds bright round blobs. A pixel is foreground when it exceeds the mean of
/// its neighborhood by a margin tied to the image contrast, which keeps dots
/// detectable across screen brightness and room lighting without a global
/// threshold.
pub(super) fn find_blobs(image: &RgbImage, intrinsics: &Intrinsics) -> Vec<Blob> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let gray = luma(image);
    let margin = foreground_margin(&gray);
    let half = (h / WINDOW_HEIGHT_DIVISOR).max(MIN_WINDOW_HALF);
    let integral = Integral::new(&gray, w, h);

    let weight = |x: u32, y: u32| {
        let g = f64::from(gray[y as usize * w + x as usize]);
        let local = integral.mean(x as usize, y as usize, half, w, h);
        if g > local + margin { g - local } else { 0.0 }
    };

    let max_area = (MAX_SIDE_HEIGHT_SHARE * h as f64).powi(2);
    let mut blobs = Vec::new();
    for c in components(w as u32, h as u32, weight) {
        if !is_dot_shaped(&c, max_area) {
            continue;
        }
        let Some(undistorted) = undistort_pixel(intrinsics, c.center) else {
            continue;
        };
        blobs.push(Blob {
            raw: c.center,
            undistorted,
            area: f64::from(c.area),
        });
    }
    blobs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 block in the interior and a 3x1 bar on the top edge.
    fn two_regions(x: u32, y: u32) -> f64 {
        let block = (3..5).contains(&x) && (2..4).contains(&y);
        let bar = (5..8).contains(&x) && y == 0;
        if block || bar { 1.0 } else { 0.0 }
    }

    #[test]
    fn components_report_centroid_box_and_border_contact() {
        let found = components(10, 8, two_regions);
        assert_eq!(found.len(), 2);

        let block = found.iter().find(|c| c.area == 4).unwrap();
        assert_eq!(block.center, Point2::new(3.5, 2.5));
        assert_eq!((block.bbox_w, block.bbox_h), (2, 2));
        assert!(!block.touches_border);
        assert!((block.axis_ratio - 1.0).abs() < 1e-12);

        let bar = found.iter().find(|c| c.area == 3).unwrap();
        assert_eq!(bar.center, Point2::new(6.0, 0.0));
        assert!(bar.touches_border);
        assert!(bar.axis_ratio < 1e-5, "a line has no minor axis");
    }

    #[test]
    fn the_weight_pulls_the_centroid_but_not_the_area() {
        let found = components(6, 3, |x, y| {
            if y == 1 && x < 3 {
                f64::from(x) + 1.0
            } else {
                0.0
            }
        });
        let [c] = found[..] else { panic!("{found:?}") };
        assert_eq!(c.area, 3);
        // Weights 1, 2, 3 at x = 0, 1, 2.
        assert_eq!(c.center, Point2::new(8.0 / 6.0, 1.0));
    }
}
