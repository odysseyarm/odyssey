//! Which of the six measured markers is which in `ScreenCalibration::object_points`.
//!
//! The markers may be anywhere on or around the screen, but the order they are stored in is
//! not free. The hub's tracker (`odyssey-cv`, `identify_markers_from_screen_calibration`)
//! takes `object_points[0..3]` as the seed of a three-point pose solve and finds the matching
//! image points by testing every triple of what the controller's camera sees:
//!
//! - `[1]` and `[2]` are the ends of a nearly straight row with exactly one other marker
//!   between them (the tracker counts the points whose angle at the middle is wider than about
//!   143 degrees);
//! - `[0]` lies below `[1]` along gravity, and on the side that makes the turn from `[1]`'s
//!   row to `[0]` counter-clockwise as the camera sees it (the cross product of `[0] - [1]`
//!   and `[2] - [1]` is not positive in the image, y down).
//!
//! `[3]`, `[4]` and `[5]` take no part in the seed; the tracker matches them by reprojection.
//! These are the tests in `foveated.rs` of `odyssey-cv`; `crates/odyssey-hub-server/tests/
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
/// so that a slightly tilted camera still sees it below.
const MIN_DROP: f64 = 0.05;
/// The sine of the angle between `[0] - [1]` and the row at which `[0]` counts as off the
/// row. The seed's three points must not be collinear; this also keeps the solve well
/// conditioned.
const MIN_OFF_ROW: f64 = 0.25;
/// A candidate for `[0]` needs at least this share of the largest triangle area on offer.
const MIN_AREA_SHARE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlacementError {
    #[error(
        "no three markers form a straight row with exactly one between the ends, with a marker below its left end"
    )]
    NoSeed,
    #[error("a marker position is not a finite number")]
    NotFinite,
}

/// The markers in tracker order, and the seed they were ordered by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerOrder {
    /// `order[n]` is the index into the input of `object_points[n]`.
    pub order: [usize; 6],
    /// The circle through `object_points[0..3]`: the seed solve is ill-conditioned for a
    /// camera whose position, projected onto the screen's plane, is on it.
    pub seed_circle: Option<Circle>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub center: Point2<f64>,
    pub radius: f64,
}

/// `-cos` of the angle at `b` between `a` and `c`: 1 when `b` lies on the segment `ac`.
fn straightness(a: Point2<f64>, b: Point2<f64>, c: Point2<f64>) -> f64 {
    let (u, v) = (a - b, c - b);
    -u.dot(&v) / (u.norm() * v.norm())
}

fn circumcircle(a: Point2<f64>, b: Point2<f64>, c: Point2<f64>) -> Option<Circle> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
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

struct Seed {
    i: usize,
    j: usize,
    k: usize,
    middle: usize,
    area: f64,
    reach: f64,
}

/// Orders `points` the way the tracker needs them.
pub fn tracker_order(points: &[Point2<f64>; 6]) -> Result<TrackerOrder, PlacementError> {
    if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return Err(PlacementError::NotFinite);
    }
    let (mut lo, mut hi) = (points[0], points[0]);
    for p in points {
        lo = Point2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let drop = MIN_DROP * (hi - lo).norm();

    let mut seeds = Vec::new();
    for j in 0..6 {
        for k in 0..6 {
            if j == k {
                continue;
            }
            let others: Vec<usize> = (0..6).filter(|&n| n != j && n != k).collect();
            let straight = |n: usize| straightness(points[j], points[n], points[k]);
            let in_row: Vec<usize> = others
                .iter()
                .copied()
                .filter(|&n| straight(n) > OTHERS_MAX_STRAIGHTNESS)
                .collect();
            let [middle] = in_row[..] else { continue };
            if straight(middle) < MIDDLE_MIN_STRAIGHTNESS {
                continue;
            }
            for &i in others.iter().filter(|&&n| n != middle) {
                let (v1, v2) = (points[i] - points[j], points[k] - points[j]);
                let cross = v1.x * v2.y - v1.y * v2.x;
                if points[i].y - points[j].y < drop
                    || cross / (v1.norm() * v2.norm()) > -MIN_OFF_ROW
                {
                    continue;
                }
                seeds.push(Seed {
                    i,
                    j,
                    k,
                    middle,
                    area: cross.abs() / 2.0,
                    reach: v1.norm(),
                });
            }
        }
    }
    let largest = seeds.iter().map(|s| s.area).fold(0.0, f64::max);
    // The most spread-out triangle solves best; among those nearly as good, the one nearest
    // the row's left end, which is what the layout the tracker was built on does.
    let seed = seeds
        .iter()
        .filter(|s| s.area >= MIN_AREA_SHARE * largest)
        .min_by(|a, b| {
            a.reach
                .total_cmp(&b.reach)
                .then((a.i, a.j, a.k).cmp(&(b.i, b.j, b.k)))
        })
        .ok_or(PlacementError::NoSeed)?;

    let mut rest: Vec<usize> = (0..6)
        .filter(|&n| ![seed.i, seed.j, seed.k, seed.middle].contains(&n))
        .collect();
    rest.sort_by(|&a, &b| {
        points[a]
            .x
            .total_cmp(&points[b].x)
            .then(points[a].y.total_cmp(&points[b].y))
    });
    let order = [seed.i, seed.j, seed.k, seed.middle, rest[0], rest[1]];
    Ok(TrackerOrder {
        order,
        seed_circle: circumcircle(points[seed.i], points[seed.j], points[seed.k]),
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
        assert!(tracker_order(&p).is_err());
    }
}
