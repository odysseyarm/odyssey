//! Which of the six measured markers is which in `ScreenCalibration::object_points`.
//!
//! The markers may be anywhere on or around the screen, but the order they are stored in is
//! not free. The hub's tracker (`odyssey-cv`, `identify_markers_from_screen_calibration`)
//! takes `object_points[0..3]` as the seed of a three-point pose solve and finds the matching
//! image points by testing every triple of what the controller's camera sees:
//!
//! - `[1]` and `[2]` are the ends of a nearly straight row with exactly one other marker
//!   between them (the tracker counts the points whose angle at the middle is wider than about
//!   143 degrees).
//! - `[0]` lies below `[1]` along gravity, and on the side that makes the turn from `[1]`'s
//!   row to `[0]` counter-clockwise as the camera sees it (the cross product of `[0] - [1]`
//!   and `[2] - [1]` is not positive in the image, y down).
//!
//! `[3]`, `[4]` and `[5]` take no part in the seed. The tracker matches them by reprojection.
//! These are the tests in `foveated.rs` of `odyssey-cv`. `crates/odyssey-hub-server/tests/
//! marker_order.rs` runs the tracker on the orders this module produces.
//!
//! Everything here is in the screen's frame, in meters, x to the right and y down as a viewer
//! in front of the screen sees it, which is also how the controller's camera sees it.

use nalgebra::Point2;

/// The tracker counts a point as between the row's ends when the angle at it is wider than
/// about 143 degrees (`straightness > 0.8`). The middle marker must clear this with room for
/// the image's perspective and for measurement error.
const MIDDLE_MIN_STRAIGHTNESS: f64 = 0.9;
/// Every other marker must stay clear of the row's line by this much, or the tracker counts
/// a second point between the ends and drops the seed.
const OTHERS_MAX_STRAIGHTNESS: f64 = 0.6;
/// How far below the row's left end `[0]` must lie, as a fraction of the markers' extent,
/// so a slightly tilted camera still sees it below.
const MIN_DROP: f64 = 0.05;
/// The sine of the angle between `[0] - [1]` and the row at which `[0]` counts as off the
/// row. The seed's three points must not be collinear, and this also keeps the solve well
/// conditioned.
const MIN_OFF_ROW: f64 = 0.25;
/// A candidate for `[0]` needs at least this share of the largest triangle area on offer.
const MIN_AREA_SHARE: f64 = 0.5;
/// Below this magnitude the determinant of three points' circle equations is zero for
/// practical purposes: the points are collinear and have no circumcircle.
const MIN_CIRCUMCIRCLE_DETERMINANT: f64 = 1e-12;

/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlacementError {
    /// Since: Odyssey Desktop 0.6.7.
    #[error(
        "no three markers form a straight row with exactly one between the ends, with a marker below its left end"
    )]
    NoSeed,
    /// Since: Odyssey Desktop 0.6.7.
    #[error("a marker position is not a finite number")]
    NotFinite,
}

/// The markers in tracker order, and the seed they were ordered by.
///
/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerOrder {
    /// `order[n]` is the index into the input of `object_points[n]`.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub order: [usize; 6],
    /// The circle through `object_points[0..3]`: the seed solve is ill-conditioned for a
    /// camera whose position, projected onto the screen's plane, is on it.
    ///
    /// Since: Odyssey Desktop 0.6.7.
    pub seed_circle: Option<Circle>,
}

/// A circle in the screen's plane.
///
/// Since: Odyssey Desktop 0.6.7.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    /// Since: Odyssey Desktop 0.6.7.
    pub center: Point2<f64>,
    /// Since: Odyssey Desktop 0.6.7.
    pub radius: f64,
}

/// `-cos` of the angle at `b` between `a` and `c`: 1 when `b` lies on the segment `ac`.
fn straightness(a: Point2<f64>, b: Point2<f64>, c: Point2<f64>) -> f64 {
    let (u, v) = (a - b, c - b);
    -u.dot(&v) / (u.norm() * v.norm())
}

/// The circle through three points, or `None` if they are (nearly) collinear.
pub(crate) fn circumcircle(a: Point2<f64>, b: Point2<f64>, c: Point2<f64>) -> Option<Circle> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < MIN_CIRCUMCIRCLE_DETERMINANT {
        return None;
    }
    let (a2, b2, c2) = (
        a.coords.norm_squared(),
        b.coords.norm_squared(),
        c.coords.norm_squared(),
    );
    let x = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let y = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = Point2::new(x, y);
    Some(Circle {
        center,
        radius: (a - center).norm(),
    })
}

/// A choice of the tracker's seed points, as indices into the markers.
struct Seed {
    /// The marker below the row, `object_points[0]`.
    off_row: usize,
    /// The end of the row that `off_row` lies below, `object_points[1]`.
    row_start: usize,
    /// The other end of the row, `object_points[2]`.
    row_end: usize,
    /// The single marker between the row's ends, `object_points[3]`.
    middle: usize,
    /// Area of the triangle of the three seed points.
    area: f64,
    /// Distance from the row's start to `off_row`.
    reach: f64,
}

/// Length of the diagonal of the markers' bounding box.
fn extent(points: &[Point2<f64>]) -> f64 {
    let (mut lo, mut hi) = (points[0], points[0]);
    for p in points {
        lo = Point2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (hi - lo).norm()
}

/// Every way to pick the row's ends, the marker between them and the marker below the row
/// that satisfies the tracker's conditions (see the module documentation).
fn candidate_seeds(points: &[Point2<f64>; 6]) -> Vec<Seed> {
    let drop = MIN_DROP * extent(points);
    let mut seeds = Vec::new();
    for row_start in 0..points.len() {
        for row_end in 0..points.len() {
            if row_start == row_end {
                continue;
            }
            let others: Vec<usize> = (0..points.len())
                .filter(|&n| n != row_start && n != row_end)
                .collect();
            let straight = |n: usize| straightness(points[row_start], points[n], points[row_end]);
            let in_row: Vec<usize> = others
                .iter()
                .copied()
                .filter(|&n| straight(n) > OTHERS_MAX_STRAIGHTNESS)
                .collect();
            let [middle] = in_row[..] else { continue };
            if straight(middle) < MIDDLE_MIN_STRAIGHTNESS {
                continue;
            }
            for &off_row in others.iter().filter(|&&n| n != middle) {
                let (v1, v2) = (
                    points[off_row] - points[row_start],
                    points[row_end] - points[row_start],
                );
                let cross = v1.x * v2.y - v1.y * v2.x;
                if points[off_row].y - points[row_start].y < drop
                    || cross / (v1.norm() * v2.norm()) > -MIN_OFF_ROW
                {
                    continue;
                }
                seeds.push(Seed {
                    off_row,
                    row_start,
                    row_end,
                    middle,
                    area: cross.abs() / 2.0,
                    reach: v1.norm(),
                });
            }
        }
    }
    seeds
}

/// The most spread-out triangle solves best. Among those nearly as good, the pick is the
/// one nearest the row's left end, as in the layout the tracker was built on.
fn best_seed(seeds: &[Seed]) -> Option<&Seed> {
    let largest = seeds.iter().map(|s| s.area).fold(0.0, f64::max);
    seeds
        .iter()
        .filter(|s| s.area >= MIN_AREA_SHARE * largest)
        .min_by(|a, b| {
            a.reach
                .total_cmp(&b.reach)
                .then((a.off_row, a.row_start, a.row_end).cmp(&(b.off_row, b.row_start, b.row_end)))
        })
}

/// Orders `points` the way the tracker needs them.
///
/// # Errors
///
/// [`PlacementError::NotFinite`] if a coordinate is NaN or infinite.
/// [`PlacementError::NoSeed`] if no choice of markers meets the seed conditions in the
/// module documentation.
///
/// # Since
/// Odyssey Desktop 0.6.7.
pub fn tracker_order(points: &[Point2<f64>; 6]) -> Result<TrackerOrder, PlacementError> {
    if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return Err(PlacementError::NotFinite);
    }
    let seeds = candidate_seeds(points);
    let seed = best_seed(&seeds).ok_or(PlacementError::NoSeed)?;

    // The tracker matches the markers outside the seed by reprojection, so any fixed order
    // will do. Left to right keeps it reproducible.
    let mut rest: Vec<usize> = (0..points.len())
        .filter(|&n| ![seed.off_row, seed.row_start, seed.row_end, seed.middle].contains(&n))
        .collect();
    rest.sort_by(|&a, &b| {
        points[a]
            .x
            .total_cmp(&points[b].x)
            .then(points[a].y.total_cmp(&points[b].y))
    });
    let order = [
        seed.off_row,
        seed.row_start,
        seed.row_end,
        seed.middle,
        rest[0],
        rest[1],
    ];
    Ok(TrackerOrder {
        order,
        seed_circle: circumcircle(
            points[seed.off_row],
            points[seed.row_start],
            points[seed.row_end],
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(f: [[f64; 2]; 6]) -> [Point2<f64>; 6] {
        f.map(|[x, y]| Point2::new(x * 1.2, y * 0.68))
    }

    /// The original autocalib's pattern, in its order.
    const EDGE: [[f64; 2]; 6] = [
        [0.2, 1.0],
        [0.2, 0.0],
        [0.8, 0.0],
        [0.5, 0.0],
        [0.5, 1.0],
        [0.8, 1.0],
    ];

    #[test]
    fn the_edge_pattern_keeps_its_own_order_whatever_order_it_is_given_in() {
        let edge = pts(EDGE);
        for shuffle in [[0, 1, 2, 3, 4, 5], [5, 3, 1, 4, 2, 0], [2, 4, 0, 5, 1, 3]] {
            let given = shuffle.map(|i| edge[i]);
            let got = tracker_order(&given).unwrap().order.map(|n| given[n]);
            assert_eq!(got, edge, "given as {shuffle:?}");
        }
    }

    #[test]
    fn a_marker_on_the_row_is_never_the_third_seed_point() {
        // Three on the top edge, one at each side lower down, one at the bottom middle.
        let p = pts([
            [0.2, 0.0],
            [0.5, 0.0],
            [0.8, 0.0],
            [0.0, 0.5],
            [1.0, 0.45],
            [0.5, 1.0],
        ]);
        let t = tracker_order(&p).unwrap();
        let [o0, o1, o2, o3, ..] = t.order;
        assert_eq!([o1, o2, o3], [0, 2, 1], "row ends, then the one between");
        assert!(
            [3, 4, 5].contains(&o0),
            "the first seed point is off the row"
        );
    }

    #[test]
    fn the_seed_circle_passes_through_the_three_seed_points() {
        let p = pts(EDGE);
        let t = tracker_order(&p).unwrap();
        let c = t.seed_circle.unwrap();
        for n in &t.order[..3] {
            assert!(((p[*n] - c.center).norm() - c.radius).abs() < 1e-9);
        }
    }

    #[test]
    fn the_circle_of_a_right_triangle_is_centered_on_its_hypotenuse() {
        let circle = circumcircle(
            Point2::new(0.0, 0.0),
            Point2::new(4.0, 0.0),
            Point2::new(0.0, 3.0),
        )
        .unwrap();
        assert_eq!(circle.center, Point2::new(2.0, 1.5));
        assert_eq!(circle.radius, 2.5);
    }

    #[test]
    fn collinear_points_have_no_circle() {
        let line = [0.0, 1.0, 2.5].map(|x| Point2::new(x, 0.5 * x));
        assert_eq!(circumcircle(line[0], line[1], line[2]), None);
    }

    #[test]
    fn no_straight_row_means_no_order() {
        // Two rows of two and a pair: no three markers in a line.
        let p = pts([
            [0.1, 0.1],
            [0.4, 0.3],
            [0.9, 0.1],
            [0.2, 0.9],
            [0.6, 0.7],
            [0.95, 0.95],
        ]);
        assert_eq!(tracker_order(&p), Err(PlacementError::NoSeed));
    }

    #[test]
    fn markers_that_cannot_sit_below_the_row_have_no_order() {
        // Every marker on one line.
        let p = pts([
            [0.1, 0.5],
            [0.2, 0.5],
            [0.3, 0.5],
            [0.5, 0.5],
            [0.7, 0.5],
            [0.9, 0.5],
        ]);
        tracker_order(&p).unwrap_err();
    }
}
