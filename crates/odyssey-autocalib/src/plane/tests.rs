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
fn homography_fits_reject_mismatched_degenerate_and_nonfinite_pairs() {
    let square = [
        Point2::new(0.0, 0.0),
        Point2::new(1.0, 0.0),
        Point2::new(1.0, 1.0),
        Point2::new(0.0, 1.0),
    ];
    assert!(fit_homography(&square, &square[..3]).is_none());
    assert!(ransac_homography(&square, &square[..3], 1.0, 10, 0).is_none());
    let line = [0.0, 1.0, 2.0, 3.0].map(|x| Point2::new(x, 2.0 * x));
    assert!(fit_homography(&line, &square).is_none());
    assert!(fit_homography(&square, &line).is_none());
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut invalid = square;
        invalid[0].x = bad;
        assert!(fit_homography(&invalid, &square).is_none());
        assert!(ransac_homography(&square, &invalid, 1.0, 10, 0).is_none());
    }
}

#[test]
fn geometric_projections_reject_nonfinite_results() {
    let intr = intrinsics();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(project(&intr, &Vector3::new(bad, 0.0, 1.0)).is_none());
        assert!(project_pinhole(&intr, &Vector3::new(0.0, 0.0, bad)).is_none());
        assert!(apply_homography(&Matrix3::identity(), Point2::new(bad, 0.0)).is_none());
    }
    let plane = Plane::new(Vector3::new(0.0, 0.0, f64::INFINITY), Vector3::z());
    assert!(
        plane
            .intersect_ray(&Vector3::zeros(), &Vector3::z())
            .is_none()
    );
}

#[test]
fn plane_fits_reject_nonfinite_clouds_and_invalid_sampling() {
    let points = [
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 1.0),
        Vector3::new(0.0, 1.0, 1.0),
    ];
    let params = RansacParams {
        sample_size: 3,
        score_points: 3,
        ..RansacParams::default()
    };
    let mut invalid = points;
    invalid[0].x = f64::NAN;
    assert!(fit_plane(&invalid).is_none());
    assert!(ransac_plane(&invalid, &params).is_none());
    assert!(
        ransac_plane(
            &points,
            &RansacParams {
                sample_size: 2,
                ..params
            }
        )
        .is_none()
    );
    assert!(
        ransac_plane(
            &points,
            &RansacParams {
                score_points: 0,
                ..params
            }
        )
        .is_none()
    );
    for threshold in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            ransac_plane(
                &points,
                &RansacParams {
                    threshold,
                    ..params
                }
            )
            .is_none()
        );
    }
}
