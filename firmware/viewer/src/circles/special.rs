use std::cmp::Ordering;

use opencv::core::{Point2f, Size, Vector};

pub fn _find_circles_grid_special_old(
    centers: &Vector<Point2f>,
    board_size: Size
) -> Option<Vector<Point2f>> {
    let expected_num_outer_points = 2 * (board_size.width + board_size.height - 2) as usize;

    // Ensure that we have enough points to form the outer boundary
    if centers.len() < expected_num_outer_points {
        return None;
    }

    // Find the top-left corner by minimizing x+y
    let mut top_left = Point2f::new(f32::MAX, f32::MAX);
    for point in centers.iter() {
        if point.x + point.y < top_left.x + top_left.y {
            top_left = point;
        }
    }

    let mut sorted_hull_points = vec![top_left];
    let mut current = top_left;
    let directions = vec![
        Point2f::new(1.0, 0.0),  // Right
        Point2f::new(0.0, 1.0),  // Down
        Point2f::new(-1.0, 0.0), // Left
        Point2f::new(0.0, -1.0), // Up
    ];
    let mut current_direction = 0;
    let threshold_distance = 20.0 * 4094.0 / 97.0;

    while sorted_hull_points.len() < expected_num_outer_points {
        let mut best_point = None;
        let mut best_distance = f64::MAX;

        for point in centers.iter() {
            let distance = (point - current).norm();
            if distance < threshold_distance && distance > 0.0 {
                let direction = point - current;
                let dot_product = direction.dot(directions[current_direction]);

                if dot_product > 0.9 && distance < best_distance {
                    best_distance = distance;
                    best_point = Some(point);
                }
            }
        }

        if let Some(best_point) = best_point {
            current = best_point;
            sorted_hull_points.push(current);
        } else {
            current_direction = (current_direction + 1) % 4;
        }

        if current_direction == 0 && best_point.is_none() {
            break;
        }
    }

    if sorted_hull_points.len() == expected_num_outer_points {
        Some(Vector::from(sorted_hull_points))
    } else {
        None
    }
}

pub fn find_circles_grid_special(
    centers: &[Point2f],
    board_size: Size
) -> Option<Vector<Point2f>> {
    let expected_num_outer_points = 2 * (board_size.width + board_size.height - 2) as usize;
    if centers.len() != expected_num_outer_points {
        return None;
    }
    let hull = convex_hull(centers.to_vec())?;
    let corners_ix = find_corners(&hull);
    if corners_ix.len() != 4 {
        return None;
    }

    let top_left = corners_ix.iter().copied().min_by_key(|&i| TotalF32(hull[i].x + hull[i].y)).unwrap();
    let top_left = hull[top_left];

    let center: Point2f =
        hull[corners_ix[0]] +
        hull[corners_ix[1]] +
        hull[corners_ix[2]] +
        hull[corners_ix[3]];
    let center = center / 4.0;

    let mut sorted_centers = centers.to_vec();
    sorted_centers.sort_by_key(|&p| {
        let v = p - center;
        TotalF32(f32::atan2(v.y, v.x))
    });
    let top_left_i = sorted_centers.iter().copied().position(|x| x == top_left).unwrap();
    sorted_centers.rotate_left(top_left_i);
    Some(sorted_centers.into())
}

/// Returns `None` if less than 3 points are provided.
pub fn convex_hull(
    points: Vec<Point2f>,
) -> Option<Vec<Point2f>> {
    if points.len() <= 2 {
        return None;
    }
    let mut points = points.to_vec();
    points.sort_by_key(|p| (TotalF32(p.x), TotalF32(p.y)));

    let mut hull = vec![points[0], points[1]];

    for &c in &points[2..] {
        while hull.len() >= 2 {
            let a = hull[hull.len() - 2];
            let b = hull[hull.len() - 1];
            if (b-a).cross(c-a) <= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(c);
    }

    let hl = hull.len();
    for &c in points.iter().rev().skip(1) {
        while hull.len() - hl + 1 >= 2 {
            let a = hull[hull.len() - 2];
            let b = hull[hull.len() - 1];
            if (b-a).cross(c-a) <= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(c);
    }
    hull.pop();
    Some(hull)
}

pub fn find_corners(
    hull: &[Point2f]
) -> Vec<usize> {
    let mut corners = vec![];
    for (i, &cur) in hull.iter().enumerate() {
        let prev = hull[(i + hull.len() - 1) % hull.len()];
        let next = hull[(i + hull.len() + 1) % hull.len()];
        let a = prev - cur;
        let b = next - cur;
        let a = a / a.norm() as f32;
        let b = b / b.norm() as f32;
        let angle = f32::acos(a.dot(b));
        if angle < 140.0_f32.to_radians() {
            corners.push(i);
        }
    }
    corners
}

struct TotalF32(f32);

impl PartialOrd for TotalF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.0.total_cmp(&other.0))
    }
}

impl Ord for TotalF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl PartialEq for TotalF32 {
    fn eq(&self, other: &Self) -> bool {
        self.0.total_cmp(&other.0) == Ordering::Equal
    }
}

impl Eq for TotalF32 {
}
