use std::f64::consts::PI;

use ccalib::{make_extrinsics, make_extrinsics_from_mat4, reproject};
use nalgebra::{matrix, Rotation3, SMatrix, Scale3, Translation3, Vector3};
use opencv::{
    calib3d::{find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING, CALIB_CB_SYMMETRIC_GRID}, core::{Mat, Point2f, Ptr, Size, ToInputArray, ToInputOutputArray, Vector, VectorToVec, BORDER_CONSTANT, CV_8UC1, CV_8UC3}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{
        destroy_all_windows, get_window_property, imshow, wait_key, wait_key_def, WND_PROP_VISIBLE
    }, imgproc::{ellipse, warp_perspective, warp_perspective_def, INTER_LINEAR, LINE_AA}
};

fn fill_circle(
    im: &mut impl ToInputOutputArray,
    center: (f64, f64),
    radius: f64,
    color: [f64; 4],
) -> opencv::Result<()> {
    let cx = (center.0 * 1024.0).round() as i32;
    let cy = (center.1 * 1024.0).round() as i32;
    let r = (radius * 1024.0).round() as i32;
    ellipse(
        im,
        (cx, cy).into(),
        (r, r).into(),
        0.0,
        0.0,
        360.0,
        color.into(),
        -1,
        LINE_AA,
        10,
    )
}

fn get_circle_centers(im: &impl ToInputArray) -> Vector<Point2f> {
    let mut centers = Vector::<Point2f>::default();

    let mut params = SimpleBlobDetector_Params::default().unwrap();
    params.min_threshold = 50.0;
    params.max_threshold = 150.0;
    params.min_area = 10.0;
    params.max_area = 10000.0;
    params.filter_by_area = true;
    params.filter_by_convexity = true;
    params.min_convexity = 0.87;
    params.min_circularity = 0.1;
    params.filter_by_circularity = true;
    params.min_inertia_ratio = 0.01;
    params.filter_by_inertia = true;

    let mut circle_grid_finder_params = CirclesGridFinderParameters::default().unwrap();
    circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::SYMMETRIC_GRID;
    let simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);

    let pattern_was_found = find_circles_grid(
        im,
        (3, 3).into(),
        &mut centers,
        CALIB_CB_SYMMETRIC_GRID | CALIB_CB_CLUSTERING,
        Some(&feature2d_detector),
        circle_grid_finder_params,
    ).unwrap();
    if pattern_was_found {
        centers
    } else {
        panic!("failed to find circle grid");
    }
}

fn main() {
    test_calib();
    return;
    // draw circles on an image
    let mut im = Mat::new_size_with_default((1000, 1000).into(), CV_8UC3, [255.0; 4].into()).unwrap();

    let circle_centers = [
        (140, 140),
        (500, 140),
        (860, 140),
        (140, 500),
        (500, 500),
        (860, 500),
        (140, 860),
        (500, 860),
        (860, 860),
    ];
    let radius = 128;

    for cc in circle_centers {
        let ccf = (cc.0 as f64, cc.1 as f64);
        fill_circle(&mut im, ccf, radius as f64, [0.0; 4]).unwrap();
        // opencv::imgproc::line(&mut im, (cc.0, 0).into(), (cc.0, 500).into(), [0., 0., 255., 0.].into(), 1, LINE_AA, 0).unwrap();
        // opencv::imgproc::line(&mut im, (0, cc.1).into(), (500, cc.1).into(), [0., 0., 255., 0.].into(), 1, LINE_AA, 0).unwrap();
    }

    // transform the circles with perspective
    let k = matrix![
        300.,   0., 250.;
          0., 300., 250.;
          0.,   0.,   1.;
    ];
    let fx = k.m11;
    let fy = k.m22;
    let cx = k.m13;
    let cy = k.m23;
    let e_iso = Translation3::new(0., 0., 1000.) * Rotation3::from_axis_angle(&Vector3::y_axis(), 38f64.to_radians()) * Translation3::new(-500., -500., 0.);
    let e = make_extrinsics_from_mat4(&e_iso.to_matrix());

    let mut cam = Mat::default();
    let mat = (k*e).transpose(); // nalgebra is column major
    let mat = Mat::new_rows_cols_with_data(3, 3, mat.as_slice()).unwrap();
    warp_perspective(&im, &mut cam, &mat, (500, 500).into(), INTER_LINEAR, BORDER_CONSTANT, [255.0; 4].into()).unwrap();

    // draw detected blob centers
    let blob_centers = get_circle_centers(&cam);
    let mut blob_centers = blob_centers.to_vec();
    blob_centers.sort_unstable_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    for p in blob_centers {
        println!("{p:?}");
        fill_circle(&mut cam, (p.x as f64, p.y as f64), 4.0, [0., 0., 255., 0.]).unwrap();
    }
    println!();

    // draw estimated blob centers
    for cc in circle_centers {
        let r = e_iso.rotation.scaled_axis();
        let t = e_iso.translation.vector;
        let p = reproject(fx, fy, cx, cy, r, t, Vector3::new(cc.0, cc.1, 0).cast(), radius as f64);
        println!("{p:?}");
        fill_circle(&mut cam, (p.x, p.y), 3.0, [0., 255., 0., 0.]).unwrap();
    }

    imshow("img", &cam).unwrap();

    loop {
        let c = wait_key(100).unwrap();
        // println!("{} {:?}", c, char::from_u32(c as u32));
        if c == b'q'.into()
            || c == 0x1b
            || get_window_property("img", WND_PROP_VISIBLE).unwrap() == 0.
        {
            // q or esc
            break;
        }
    }
    destroy_all_windows().unwrap();
}

fn test_calib() {
    let images_points = [
        SMatrix::<f64, 2, 9>::from([
            [102.92784, 141.48114],
            [250.00058, 141.4969],
            [397.07214, 141.48114],
            [141.12552, 247.49763],
            [250.0, 247.50888],
            [358.8746, 247.4984],
            [163.47325, 309.60333],
            [250.0, 309.61908],
            [336.52673, 309.60333],
        ]),
        SMatrix::from([
            [163.47325, 190.39667],
            [250.0, 190.38094],
            [336.52673, 190.39667],
            [141.12538, 252.5016],
            [250.0, 252.49112],
            [358.87448, 252.50237],
            [102.92801, 358.51938],
            [250.00035, 358.50385],
            [397.0727, 358.51938],
        ]),
        SMatrix::from([
            [132.02682, 117.48052],
            [247.87334, 141.54811],
            [327.91635, 158.22177],
            [132.01678, 250.00052],
            [247.83434, 250.0],
            [327.91232, 249.9993],
            [132.02638, 382.52005],
            [247.87334, 358.4519],
            [327.91635, 341.77823],
        ]),
        SMatrix::from([
            [181.67229, 161.26385],
            [252.4077, 141.30884],
            [364.46527, 109.80811],

            [181.69334, 250.0],
            [252.37836, 250.0],
            [364.48203, 250.0],

            [181.67229, 338.73615],
            [252.4077, 358.69116],
            [364.4649, 390.19128],
        ]),
    ];
    let circle_centers = [
        Vector3::new(140., 140., 0.),
        Vector3::new(500., 140., 0.),
        Vector3::new(860., 140., 0.),
        Vector3::new(140., 500., 0.),
        Vector3::new(500., 500., 0.),
        Vector3::new(860., 500., 0.),
        Vector3::new(140., 860., 0.),
        Vector3::new(500., 860., 0.),
        Vector3::new(860., 860., 0.),
    ];
    let radius = 128.0;
    ccalib::calibrate(
        &circle_centers,
        &images_points,
        radius,
        300.0,
        250.0,
        250.0,
        [-500., -500., 1000.].into(),
    );
}
