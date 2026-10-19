//! Whether the hub's tracker identifies the markers in the order the calibration wizard stores
//! them.
//!
//! The wizard writes the six markers' positions into `ScreenCalibration::object_points`, in an
//! order it picks (`odyssey_autocalib::placement::tracker_order`). The tracker (`odyssey-cv`) uses
//! the first three as the seed of a pose solve and finds the matching points in the controller's
//! camera by testing every triple. These tests build a calibration, project the markers through
//! a camera that faces the screen, and run the tracker's own
//! `identify_markers_from_screen_calibration`, so the order is one the tracker accepts and not one
//! we assumed.

use nalgebra::{Matrix3, Point2, Point3, Rotation3, UnitQuaternion, UnitVector3, Vector3};
use odyssey_autocalib::placement::{PlacementError, tracker_order};
use odyssey_common::ScreenCalibration;
use odyssey_cv::foveated::identify_markers_from_screen_calibration;

/// A 55 inch 16:9 screen, in meters.
const W: f64 = 1.2;
const H: f64 = 0.68;

/// Where the controller is: the foot of its perpendicular on the screen's plane, in meters from
/// the screen's top-left corner (x right, y down), its distance in front of the screen, and its
/// roll about the line of sight in radians.
#[derive(Clone, Copy)]
struct View {
    x: f64,
    y: f64,
    distance: f64,
    roll: f64,
}

fn calibration(points: &[Point2<f64>; 6]) -> ScreenCalibration<f32> {
    ScreenCalibration {
        rotation: UnitQuaternion::identity(),
        homography: Matrix3::identity(),
        object_points: points.map(|p| Point3::new(p.x as f32, p.y as f32, 0.0)),
    }
}

/// What the camera sees of `points`, in normalized image coordinates, listed in `SHUFFLE` order,
/// and the gravity vector in the camera's frame.
fn observe(points: &[Point2<f64>; 6], view: View) -> (Vec<Point2<f32>>, UnitVector3<f32>) {
    let r = Rotation3::from_axis_angle(&Vector3::z_axis(), view.roll as f32);
    let camera_at = Vector3::new(view.x as f32, view.y as f32, -view.distance as f32);
    let seen: Vec<Point2<f32>> = points
        .iter()
        .map(|p| {
            let at = r * (Vector3::new(p.x as f32, p.y as f32, 0.0) - camera_at);
            Point2::new(at.x / at.z, at.y / at.z)
        })
        .collect();
    let gravity = UnitVector3::new_normalize(r * Vector3::y());
    (SHUFFLE.map(|i| seen[i]).to_vec(), gravity)
}

/// The order the camera reports the points in, so that matching cannot lean on it.
const SHUFFLE: [usize; 6] = [3, 0, 5, 1, 4, 2];

/// Whether the tracker names every marker correctly in a view of `points`, listed in the order
/// stored in the calibration.
fn identified(points: &[Point2<f64>; 6], view: View) -> bool {
    let (image, gravity) = observe(points, view);
    match identify_markers_from_screen_calibration(&image, gravity, &calibration(points), 0.0) {
        Some((matched, _, _)) => (0..6).all(|marker| SHUFFLE[matched[marker]] == marker),
        None => false,
    }
}

/// Positions in front of a screen where a controller is used, at several distances and rolls.
fn views() -> Vec<View> {
    let mut v = Vec::new();
    for (x, y) in [(0.5, 0.9), (0.2, 1.1), (0.8, 1.1), (0.5, 0.5), (0.5, 1.5)] {
        for distance in [1.5, 2.5, 4.0] {
            for roll in [0.0, 0.1, -0.1] {
                v.push(View {
                    x: x * W,
                    y: y * H,
                    distance,
                    roll,
                });
            }
        }
    }
    v
}

fn in_every_view(points: &[Point2<f64>; 6]) -> bool {
    views().iter().all(|v| identified(points, *v))
}

fn meters(fractions: [[f64; 2]; 6]) -> [Point2<f64>; 6] {
    fractions.map(|[u, v]| Point2::new(u * W, v * H))
}

/// The original autocalib's pattern (`screen-autocalib/mathstuff.py`, `marker_pattern`), in its
/// order: three along the top edge, three along the bottom.
const EDGE: [[f64; 2]; 6] = [
    [0.2, 1.0],
    [0.2, 0.0],
    [0.8, 0.0],
    [0.5, 0.0],
    [0.5, 1.0],
    [0.8, 1.0],
];

/// Where the six markers of a TV with markers on its bezel were measured: three along the top
/// edge, two on the right and one on the left (from a wizard log, as fractions of the screen).
const BEZEL: [[f64; 2]; 6] = [
    [0.22, -0.04],
    [0.52, -0.03],
    [0.80, -0.04],
    [1.01, 0.08],
    [1.02, 0.42],
    [-0.02, 0.38],
];

/// The wizard's order for `points`, which must exist.
fn wizard_order(points: &[Point2<f64>; 6]) -> [Point2<f64>; 6] {
    tracker_order(points)
        .expect("an order")
        .order
        .map(|n| points[n])
}

/// A small deterministic jitter, in meters, so no arrangement is exact.
fn jitter(points: [Point2<f64>; 6], amplitude: f64) -> [Point2<f64>; 6] {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    points.map(|p| Point2::new(p.x + amplitude * next(), p.y + amplitude * next()))
}

#[test]
fn the_original_pattern_in_its_own_order_is_identified_everywhere() {
    assert!(in_every_view(&meters(EDGE)));
}

#[test]
fn the_row_ends_come_first_and_third_in_the_tracker_s_order() {
    // The order is the contract: [1] and [2] are the ends of the row, with one marker between;
    // swapping them, or putting the middle marker first, is not identified.
    let edge = meters(EDGE);
    let mut swapped = edge;
    swapped.swap(1, 2);
    assert!(!in_every_view(&swapped));
    let mut middle_first = edge;
    middle_first.swap(0, 3);
    assert!(!in_every_view(&middle_first));
}

#[test]
fn whatever_order_the_markers_are_found_in_the_wizard_stores_one_the_tracker_accepts() {
    let edge = meters(EDGE);
    for shuffle in [[0, 1, 2, 3, 4, 5], [5, 3, 1, 4, 2, 0], [2, 4, 0, 5, 1, 3]] {
        let found = shuffle.map(|i| edge[i]);
        assert!(in_every_view(&wizard_order(&found)), "found as {shuffle:?}");
    }
}

#[test]
fn markers_on_a_bezel_are_stored_in_an_accepted_order() {
    let bezel = meters(BEZEL);
    assert!(in_every_view(&wizard_order(&bezel)));
    // Measured positions are never exact.
    for amplitude in [0.002, 0.005] {
        let measured = jitter(bezel, amplitude);
        assert!(
            in_every_view(&wizard_order(&measured)),
            "{amplitude} m of error"
        );
    }
}

#[test]
fn a_mirrored_or_moved_arrangement_is_also_accepted() {
    let mirrored = meters(EDGE.map(|[u, v]| [1.0 - u, v]));
    assert!(in_every_view(&wizard_order(&mirrored)));
    // Behind a projector screen: well inside the picture, and not on a perfect grid.
    let behind = meters([
        [0.15, 0.12],
        [0.52, 0.10],
        [0.86, 0.13],
        [0.14, 0.88],
        [0.47, 0.90],
        [0.83, 0.86],
    ]);
    assert!(in_every_view(&wizard_order(&behind)));
}

#[test]
fn without_a_row_with_a_marker_below_it_neither_the_wizard_nor_the_tracker_can_start() {
    // A row along the bottom, with the other three scattered above it (no three of the others
    // are near a line): there is nothing below the row for the third seed point.
    let bottom_row = meters([
        [0.2, 1.0],
        [0.5, 1.0],
        [0.8, 1.0],
        [0.79, 0.27],
        [0.24, 0.38],
        [0.63, 0.08],
    ]);
    assert_eq!(tracker_order(&bottom_row), Err(PlacementError::NoSeed));
    // No order of these six is identified by the tracker either, so refusing was right.
    let mut order = [0, 1, 2, 3, 4, 5];
    let mut accepted = 0;
    permute(&mut order, 0, &mut |o| {
        let ordered = o.map(|i| bottom_row[i]);
        if views().iter().any(|v| identified(&ordered, *v)) {
            accepted += 1;
        }
    });
    assert_eq!(accepted, 0);
}

fn permute(items: &mut [usize; 6], from: usize, visit: &mut impl FnMut([usize; 6])) {
    if from == 6 {
        visit(*items);
        return;
    }
    for i in from..6 {
        items.swap(from, i);
        permute(items, from + 1, visit);
        items.swap(from, i);
    }
}

#[test]
fn standing_on_the_seed_circle_is_a_weak_spot_not_a_limit() {
    // The seed solve uses markers 1, 2 and 3. A controller whose position straight in front of
    // the screen is on the circle through them is on the "danger cylinder" of a three point
    // solve. The tracker checks every start against all six markers, so it usually still locks
    // on, and only in a thin band around the circle; the guidance in the app says so, and this
    // holds it to that.
    let points = meters(EDGE);
    let circle = tracker_order(&points).unwrap().seed_circle.unwrap();
    let around = |scale: f64| {
        let (mut tried, mut found) = (0, 0);
        for distance in [1.5, 2.5, 4.0] {
            for degrees in (0..360).step_by(30) {
                let a = f64::from(degrees).to_radians();
                let view = View {
                    x: circle.center.x + scale * circle.radius * a.cos(),
                    y: circle.center.y + scale * circle.radius * a.sin(),
                    distance,
                    roll: 0.0,
                };
                tried += 1;
                found += usize::from(identified(&points, view));
            }
        }
        (found, tried)
    };
    let (on, tried) = around(1.0);
    assert!(on * 4 >= tried * 3, "{on} of {tried} on the circle");
    for scale in [0.6, 1.15] {
        let (found, tried) = around(scale);
        assert_eq!(found, tried, "at {scale} of the radius");
    }
}
