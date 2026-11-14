//! Measured display surface stored with a screen calibration.
//!
//! The homography stays the planar fit. A surface grid, when present, is the
//! display as measured: `columns * rows` vertices in physical-screen meters, row
//! major, top row first. Vertex `(c, r)` shows normalized screen UV
//! `(c / (columns - 1), r / (rows - 1))`. Each cell splits into the TL/TR/BR and
//! TL/BR/BL triangles. Uneven XY spacing is a nonlinear projector mapping. Z is
//! surface waviness.

use nalgebra::{Matrix3, Point2, Vector3};

/// Since: Odyssey Desktop 0.7.0.
pub const MAX_GRID_DIMENSION: u32 = 256;
/// Since: Odyssey Desktop 0.7.0.
pub const MAX_GRID_VERTICES: usize = 65_536;
const UV_EPSILON: f64 = 1e-10;

/// Since: Odyssey Desktop 0.7.0.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenSurface {
    /// Since: Odyssey Desktop 0.7.0.
    pub columns: u32,
    /// Since: Odyssey Desktop 0.7.0.
    pub rows: u32,
    /// Since: Odyssey Desktop 0.7.0.
    pub positions_m: Vec<[f64; 3]>,
}

/// Why a surface grid is unusable. All are fatal for that calibration.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScreenSurfaceError {
    /// Dimensions outside `2..=256`, too many vertices, or a vertex count that
    /// does not match `columns * rows`.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Dimensions,
    /// A vertex coordinate is NaN or infinite.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    NonFinite,
    /// A triangle is degenerate or flipped in XY, or the outline crosses itself.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Folded,
}

impl core::fmt::Display for ScreenSurfaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Dimensions => "surface grid must be 2..=256 per side and match its vertex count",
            Self::NonFinite => "surface grid positions must be finite",
            Self::Folded => "surface grid triangles must not be degenerate, flipped or overlapping",
        })
    }
}

impl core::error::Error for ScreenSurfaceError {}

/// Nearest forward hit on a display, in physical-screen space.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    /// Normalized screen coordinates, inside `[0, 1]^2`.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub uv: Point2<f64>,
    /// Along the unit-length ray, in meters.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub distance_m: f64,
}

impl ScreenSurface {
    /// Since: Odyssey Desktop 0.7.0.
    /// # Errors
    /// Returns an error when grid dimensions, positions, or triangles are invalid.
    ///
    pub fn validate(&self) -> Result<(), ScreenSurfaceError> {
        let (columns, rows, positions) = (self.columns, self.rows, &self.positions_m);
        let count = columns
            .checked_mul(rows)
            .and_then(|n| usize::try_from(n).ok());
        if !(2..=MAX_GRID_DIMENSION).contains(&columns)
            || !(2..=MAX_GRID_DIMENSION).contains(&rows)
            || count.is_none_or(|n| n > MAX_GRID_VERTICES || n != positions.len())
        {
            return Err(ScreenSurfaceError::Dimensions);
        }
        if positions.iter().flatten().any(|v| !v.is_finite()) {
            return Err(ScreenSurfaceError::NonFinite);
        }
        let columns = columns as usize;
        let rows = rows as usize;
        let mut orientation = None;
        for [a, b, c] in self.triangles() {
            let signed_area = orient(positions[a], positions[b], positions[c]);
            let ab = Vector3::from(positions[b]) - Vector3::from(positions[a]);
            let ac = Vector3::from(positions[c]) - Vector3::from(positions[a]);
            let xy_scale = ab.x.hypot(ab.y) * ac.x.hypot(ac.y);
            if !signed_area.is_finite()
                || !xy_scale.is_finite()
                || signed_area.abs() <= 1e-12 * xy_scale
                || orientation.is_some_and(|positive| positive != (signed_area > 0.0))
            {
                return Err(ScreenSurfaceError::Folded);
            }
            orientation = Some(signed_area > 0.0);
        }
        // Consistent local winding alone permits globally wrapped meshes. A simple
        // outer boundary plus nonzero local orientation rules out global XY overlap.
        let boundary_len = 2 * (columns + rows - 2);
        let boundary = |mut i: usize| -> [f64; 3] {
            if i < columns - 1 {
                return positions[i];
            }
            i -= columns - 1;
            if i < rows - 1 {
                return positions[i * columns + columns - 1];
            }
            i -= rows - 1;
            if i < columns - 1 {
                return positions[(rows - 1) * columns + columns - 1 - i];
            }
            i -= columns - 1;
            positions[(rows - 1 - i) * columns]
        };
        for i in 0..boundary_len {
            for j in i + 2..boundary_len {
                if i == 0 && j == boundary_len - 1 {
                    continue;
                }
                if segments_intersect(
                    boundary(i),
                    boundary((i + 1) % boundary_len),
                    boundary(j),
                    boundary((j + 1) % boundary_len),
                ) {
                    return Err(ScreenSurfaceError::Folded);
                }
            }
        }
        Ok(())
    }

    /// Vertex index triples, in the fixed split every consumer must use.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn triangles(&self) -> impl Iterator<Item = [usize; 3]> + '_ {
        let columns = self.columns as usize;
        let rows = self.rows as usize;
        (0..rows.saturating_sub(1)).flat_map(move |row| {
            (0..columns.saturating_sub(1)).flat_map(move |column| {
                let tl = row * columns + column;
                [
                    [tl, tl + 1, tl + columns + 1],
                    [tl, tl + columns + 1, tl + columns],
                ]
            })
        })
    }

    /// Physical-screen point shown at `uv`. `None` outside `[0, 1]^2`.
    /// Assumes a validated surface.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn point_at_uv(&self, uv: Point2<f64>) -> Option<Vector3<f64>> {
        if !uv.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
            return None;
        }
        let columns = self.columns as usize;
        let rows = self.rows as usize;
        let x = uv.x * (columns - 1) as f64;
        let y = uv.y * (rows - 1) as f64;
        let column = (x.floor() as usize).min(columns - 2);
        let row = (y.floor() as usize).min(rows - 2);
        let u = x - column as f64;
        let v = y - row as f64;
        let tl = row * columns + column;
        let (indices, weights) = if v <= u {
            ([tl, tl + 1, tl + columns + 1], [1.0 - u, u - v, v])
        } else {
            ([tl, tl + columns + 1, tl + columns], [1.0 - v, u, v - u])
        };
        Some(
            indices
                .iter()
                .zip(weights)
                .map(|(&i, w)| Vector3::from(self.positions_m[i]) * w)
                .sum(),
        )
    }

    /// Nearest two-sided forward hit of a physical-screen-space ray.
    /// `direction` need not be unit length. Assumes a validated surface.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn raycast(&self, origin: Vector3<f64>, direction: Vector3<f64>) -> Option<SurfaceHit> {
        let direction = direction.try_normalize(0.0)?;
        let columns = self.columns as usize;
        let rows = self.rows as usize;
        let mut nearest: Option<SurfaceHit> = None;
        for indices in self.triangles() {
            let [a, b, c] = indices.map(|i| Vector3::from(self.positions_m[i]));
            let Some((distance_m, weights)) = triangle_hit(origin, direction, a, b, c) else {
                continue;
            };
            if nearest.is_some_and(|hit| hit.distance_m <= distance_m) {
                continue;
            }
            let mut uv = Point2::origin();
            for (index, weight) in indices.into_iter().zip(weights) {
                uv.x += weight * (index % columns) as f64 / (columns - 1) as f64;
                uv.y += weight * (index / columns) as f64 / (rows - 1) as f64;
            }
            if let Some(uv) = bounded_uv(uv) {
                nearest = Some(SurfaceHit { uv, distance_m });
            }
        }
        nearest
    }
}

/// Plane `z = 0` mapped through `homography`, bounded to `[0, 1]^2`.
pub(crate) fn plane_hit(
    homography: &Matrix3<f64>,
    origin: Vector3<f64>,
    direction: Vector3<f64>,
) -> Option<SurfaceHit> {
    let direction = direction.try_normalize(0.0)?;
    let distance_m = -origin.z / direction.z;
    if !distance_m.is_finite() || distance_m < 0.0 {
        return None;
    }
    let p = origin + direction * distance_m;
    let q = homography * Vector3::new(p.x, p.y, 1.0);
    bounded_uv(Point2::new(q.x / q.z, q.y / q.z)).map(|uv| SurfaceHit { uv, distance_m })
}

fn bounded_uv(uv: Point2<f64>) -> Option<Point2<f64>> {
    uv.iter()
        .all(|v| v.is_finite() && (-UV_EPSILON..=1.0 + UV_EPSILON).contains(v))
        .then(|| uv.map(|v| v.clamp(0.0, 1.0)))
}

/// Möller–Trumbore. Returns distance along `direction` and barycentric weights
/// of `a`, `b`, `c`. `direction` should be unit length for meters.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn triangle_hit(
    origin: Vector3<f64>,
    direction: Vector3<f64>,
    a: Vector3<f64>,
    b: Vector3<f64>,
    c: Vector3<f64>,
) -> Option<(f64, [f64; 3])> {
    let ab = b - a;
    let ac = c - a;
    let cross = direction.cross(&ac);
    let determinant = ab.dot(&cross);
    if determinant.abs() <= 1e-12 * ab.norm() * ac.norm() {
        return None;
    }
    let offset = origin - a;
    let u = offset.dot(&cross) / determinant;
    let q = offset.cross(&ab);
    let v = direction.dot(&q) / determinant;
    let distance = ac.dot(&q) / determinant;
    if u < -UV_EPSILON
        || v < -UV_EPSILON
        || u + v > 1.0 + UV_EPSILON
        || distance < 0.0
        || !distance.is_finite()
    {
        return None;
    }
    Some((distance, [1.0 - u - v, u, v]))
}

fn orient(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn segments_intersect(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) -> bool {
    if (0..2).any(|i| a[i].max(b[i]) < c[i].min(d[i]) || c[i].max(d[i]) < a[i].min(b[i])) {
        return false;
    }
    let crosses =
        |x: f64, y: f64| x == 0.0 || y == 0.0 || x.is_sign_positive() != y.is_sign_positive();
    crosses(orient(a, b, c), orient(a, b, d)) && crosses(orient(c, d, a), orient(c, d, b))
}
