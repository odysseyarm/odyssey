use rand::Rng as _;

use super::*;

fn intrinsics() -> Intrinsics {
    Intrinsics {
        width: 1280,
        height: 720,
        fx: 900.0,
        fy: 905.0,
        cx: 641.0,
        cy: 358.0,
        k: [-0.08, 0.02, -0.003, 0.0, 0.0, 0.0],
        p: [0.0008, -0.0005],
    }
}

#[test]
fn distortion_round_trips_across_the_image() {
    let intr = intrinsics();
    for &(u, v) in &[
        (0.0, 0.0),
        (1279.0, 719.0),
        (640.0, 360.0),
        (10.5, 700.2),
        (1200.0, 30.0),
    ] {
        let pixel = Point2::new(u, v);
        let ray = pixel_ray(&intr, pixel).unwrap();
        let back = project(&intr, &(ray * 2.5)).unwrap();
        assert!((back - pixel).norm() < 0.02, "{pixel:?} -> {back:?}");
    }
}

#[test]
fn undistort_pixel_matches_pinhole_projection() {
    let intr = intrinsics();
    let p = Vector3::new(0.4, -0.2, 2.0);
    let distorted = project(&intr, &p).unwrap();
    let pinhole = project_pinhole(&intr, &p).unwrap();
    let undistorted = undistort_pixel(&intr, distorted).unwrap();
    assert!((undistorted - pinhole).norm() < 0.02);
}

#[test]
fn plane_fit_recovers_known_plane_with_outliers() {
    let truth = Plane::new(Vector3::new(0.1, -0.05, 2.0), Vector3::new(0.2, -0.1, -1.0));
    let mut rng = StdRng::seed_from_u64(7);
    let u = truth.normal.cross(&Vector3::x()).normalize();
    let v = truth.normal.cross(&u);
    let mut points = Vec::new();
    for _ in 0..4000 {
        let (a, b): (f64, f64) = (rng.random_range(-0.6..0.6), rng.random_range(-0.4..0.4));
        let noise: f64 = rng.random_range(-0.002..0.002);
        points.push(truth.point + u * a + v * b + truth.normal * noise);
    }
    for _ in 0..200 {
        points.push(Vector3::new(
            rng.random_range(-1.0..1.0),
            rng.random_range(-1.0..1.0),
            rng.random_range(1.0..3.0),
        ));
    }
    let fit = ransac_plane(&points, &RansacParams::default()).unwrap();
    assert!(
        fit.plane.normal.dot(&fit.plane.point) > 0.0,
        "normal points away from the camera"
    );
    assert!(fit.plane.normal.dot(&truth.normal).abs() > 1.0 - 1e-5);
    assert!(fit.plane.signed_distance(&truth.point).abs() < 5e-4);
    assert!(fit.rmse < 0.0015, "rmse {}", fit.rmse);
    assert!(fit.inliers.len() >= 4000 - 20);
}

#[test]
fn ray_plane_intersection_is_exact() {
    let plane = Plane::new(Vector3::new(0.0, 0.0, 3.0), Vector3::new(0.0, 0.3, 1.0));
    let dir = Vector3::new(0.12, -0.2, 1.0);
    let hit = plane.intersect_ray(&Vector3::zeros(), &dir).unwrap();
    assert!(plane.signed_distance(&hit).abs() < 1e-12);
    assert!((hit.xy() / hit.z - dir.xy()).norm() < 1e-12);
    assert!(
        plane
            .intersect_ray(&Vector3::zeros(), &Vector3::new(1.0, 0.0, 0.0))
            .is_none()
    );
    assert!(
        plane
            .intersect_ray(&Vector3::zeros(), &Vector3::new(0.0, 0.0, -1.0))
            .is_none()
    );
}

fn truth_homography() -> Matrix3<f64> {
    Matrix3::new(120.0, 14.0, 300.0, -9.0, 95.0, 120.0, 0.0004, 0.0002, 1.0)
}

#[test]
fn dlt_recovers_exact_homography() {
    let h = truth_homography();
    let src: Vec<_> = (0..6)
        .flat_map(|i| (0..4).map(move |j| Point2::new(f64::from(i) * 0.2, f64::from(j) * 0.3)))
        .collect();
    let dst: Vec<_> = src
        .iter()
        .map(|p| apply_homography(&h, *p).unwrap())
        .collect();
    let fit = fit_homography(&src, &dst).unwrap();
    for p in &src {
        let a = apply_homography(&h, *p).unwrap();
        let b = apply_homography(&fit, *p).unwrap();
        assert!((a - b).norm() < 1e-8);
    }
}

#[test]
fn dlt_with_four_points_matches_truth() {
    let h = truth_homography();
    let src = [
        Point2::new(0.0, 0.0),
        Point2::new(1.0, 0.0),
        Point2::new(1.0, 1.0),
        Point2::new(0.0, 1.0),
    ];
    let dst = src.map(|p| apply_homography(&h, p).unwrap());
    let fit = fit_homography(&src, &dst).unwrap();
    let probe = Point2::new(0.37, 0.61);
    let a = apply_homography(&h, probe).unwrap();
    let b = apply_homography(&fit, probe).unwrap();
    assert!((a - b).norm() < 1e-8);
}

#[test]
fn ransac_homography_rejects_gross_outliers() {
    let h = truth_homography();
    let mut rng = StdRng::seed_from_u64(3);
    let mut src = Vec::new();
    let mut dst = Vec::new();
    for i in 0..15 {
        for j in 0..9 {
            let p = Point2::new(f64::from(i) / 14.0, f64::from(j) / 8.0);
            let mut q = apply_homography(&h, p).unwrap();
            q += Vector2::new(rng.random_range(-0.1..0.1), rng.random_range(-0.1..0.1));
            src.push(p);
            dst.push(q);
        }
    }
    let bad = [3usize, 17, 40, 41, 77, 100];
    for &i in &bad {
        dst[i] += Vector2::new(35.0, -22.0);
    }
    let fit = ransac_homography(&src, &dst, 1.0, 500, 11).unwrap();
    for &i in &bad {
        assert!(!fit.inliers.contains(&i), "outlier {i} accepted");
    }
    assert_eq!(fit.inliers.len(), src.len() - bad.len());
    let probe = Point2::new(0.5, 0.5);
    let err =
        (apply_homography(&fit.h, probe).unwrap() - apply_homography(&h, probe).unwrap()).norm();
    assert!(err < 0.2, "err {err}");
    assert!(fit.rmse < 0.15);
}

#[test]
fn ransac_trial_budget_follows_the_inlier_share() {
    // Every sample is outlier-free: stop at the current trial.
    assert_eq!(trials_for_confidence(1.0, 4, 0.99, 7, 300), 7);
    // No support at all: the full budget.
    assert_eq!(trials_for_confidence(0.0, 4, 0.99, 7, 300), 300);
    // Half inliers, samples of four: ln(0.01) / ln(1 - 0.5^4) = 71.4 trials.
    assert_eq!(trials_for_confidence(0.5, 4, 0.99, 1, 300), 72);
    // The sample may be drawn more than once, never zero times.
    assert_eq!(trials_for_confidence(0.99, 4, 0.5, 1, 300), 1);
}
