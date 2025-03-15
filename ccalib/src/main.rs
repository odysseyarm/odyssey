use std::iter::zip;

use ccalib::make_extrinsics;
use nalgebra::{matrix, vector, Const, Dyn, Matrix3, OMatrix, U2};
use opencv::{
    calib3d::{find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING, CALIB_CB_SYMMETRIC_GRID}, core::{KeyPointTraitConst as _, Mat, MatTraitConst, MatTraitConstManual, MatTraitManual, Point2f, Ptr, Size, ToInputArray, ToInputOutputArray, Vec3b, Vector, _InputArrayTraitConst as _, BORDER_CONSTANT, CV_8UC1}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{
        destroy_window, get_window_property, imshow, set_window_title, wait_key, WND_PROP_VISIBLE
    }, imgcodecs::imread_def, imgproc::{ellipse, resize, resize_def, warp_perspective, INTER_LINEAR, INTER_NEAREST, LINE_AA}, prelude::Feature2DTrait
};

fn imshow_multi<T: ToInputArray>(imgs: &[T], title: &str) {
    let mut i = 0;
    let mut current_title = format!("{title} (Image {} of {})", i+1, imgs.len());
    imshow("imshow_multi", &imgs[0]).unwrap();
    set_window_title("imshow_multi", &current_title).unwrap();
    loop {
        let c = wait_key(100).unwrap();
        // println!("{} {:?}", c, char::from_u32(c as u32));
        if c == b'q'.into()
            || c == 0x1b // esc
            || get_window_property(&"imshow_multi", WND_PROP_VISIBLE).unwrap() == 0.
        {
            break;
        }

        'change_image: {
            // left arrow
            if c == 81 || c == b'h'.into() || c == b','.into() {
                i = (i + imgs.len() - 1) % imgs.len();
            } else if c == 83 || c == b'l'.into() || c == b'.'.into() {
                i = (i + 1) % imgs.len();
            } else {
                break 'change_image;
            }
            imshow(&"imshow_multi", &imgs[i].input_array().unwrap()).unwrap();
            current_title = format!("{title} (Image {} of {})", i+1, imgs.len());
            set_window_title("imshow_multi", &current_title).unwrap();
        }
    }
    destroy_window("imshow_multi").unwrap();
}

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

fn stroke_circle(
    im: &mut impl ToInputOutputArray,
    center: (f64, f64),
    radius: f64,
    color: [f64; 4],
    thickness: i32,
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
        thickness,
        LINE_AA,
        10,
    )
}

fn get_circle_centers(im: &impl ToInputArray, rows: usize, cols: usize) -> Vector<Point2f> {
    let mut centers = Vector::<Point2f>::default();

    let mut params = SimpleBlobDetector_Params::default().unwrap();
    // params.min_threshold = 125.0;
    // params.max_threshold = 127.0;
    // params.threshold_step = 1.0;
    // params.min_area = 80.0;
    // params.max_area = 10000.0;
    // params.filter_by_area = false;
    // params.filter_by_convexity = false;
    // params.min_convexity = 0.87;
    // params.min_circularity = 0.2;
    // params.filter_by_circularity = false;
    // params.min_inertia_ratio = 0.1;
    // params.filter_by_inertia = false;

    let mut circle_grid_finder_params = CirclesGridFinderParameters::default().unwrap();
    circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;
    let mut simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);

    let pattern_was_found = find_circles_grid(
        im,
        (rows as i32, cols as i32).into(),
        &mut centers,
        CALIB_CB_ASYMMETRIC_GRID | CALIB_CB_CLUSTERING,
        Some(&feature2d_detector),
        circle_grid_finder_params,
    ).unwrap();
    if pattern_was_found {
        for c in centers.as_mut_slice() {
            *c *= 4094.0 / 97.0;
        }
        centers
    } else {
        panic!("failed to find circle grid");
    }
}

fn overlay_circles(
    dst: &mut Mat,
    circle_centers: &[(f64, f64)],
    radius: f64,
    k: Matrix3<f64>,
    e: Matrix3<f64>,
) {
    let canvas_scale = 10_000.0;
    let output_size = Size::new(800, 800);
    let og_size = dst.size().unwrap();
    let output_scale_x = output_size.width as f64 / og_size.width as f64;
    let output_scale_y = output_size.height as f64 / og_size.height as f64;
    let mut im = Mat::new_size_with_default((1000, 1000).into(), CV_8UC1, [0.; 4].into()).unwrap();
    for cc in circle_centers {
        let cc = (cc.0 * canvas_scale, cc.1 * canvas_scale);
        stroke_circle(&mut im, cc, radius * canvas_scale, [255.; 4], 2).unwrap();
    }

    let mut cam = Mat::default();
    let canvas_scale_mat = Matrix3::from_diagonal(&[1.0/canvas_scale, 1.0/canvas_scale, 1.0].into());
    let output_scale_mat = matrix![
        1.0, 0.0, -0.5;
        0.0, 1.0, -0.5;
        0.0, 0.0, 1.0;
    ] * matrix![
        output_scale_x, 0.0, 0.0;
        0.0, output_scale_y, 0.0;
        0.0, 0.0, 1.0;
    ] * matrix![
        1.0, 0.0, 0.5;
        0.0, 1.0, 0.5;
        0.0, 0.0, 1.0;
    ] * matrix![
        97.0/4094.0, 0.0, 0.0;
        0.0, 97.0/4094.0, 0.0;
        0.0, 0.0, 1.0;
    ];
    let mat = (output_scale_mat*k*e*canvas_scale_mat).transpose(); // nalgebra is column major
    let mat = Mat::new_rows_cols_with_data(3, 3, mat.as_slice()).unwrap();
    resize(&dst.clone(), dst, output_size, 0.0, 0.0, INTER_NEAREST).unwrap();
    warp_perspective(&im, &mut cam, &mat, output_size, INTER_LINEAR, BORDER_CONSTANT, [0.; 4].into()).unwrap();

    let cam_data = cam.data_bytes().unwrap();
    let dst_data = dst.data_typed_mut::<Vec3b>().unwrap();
    for (cam_p, dst_p) in zip(cam_data, dst_data) {
        let c = *cam_p as f32 / 255.;
        dst_p.0[0] = (dst_p.0[0] as f32*(1.-c)) as u8;
        dst_p.0[1] = (dst_p.0[1] as f32*(1.-c)) as u8;
        dst_p.0[2] = (dst_p.0[2] as f32*(1.-c)) as u8 + *cam_p;
    }
}

fn main() {
    // ===== read images =====
    let mut circle_centers = vec![];
    let radius = 0.006; // 6 mm
    let diag_sp = 0.015; // 15 mm
    let board_rows = 5;
    let board_cols = 2;
    let vert_len = (2.0f64*diag_sp*diag_sp).sqrt() / 2.0;
    for c in 0..board_cols {
        for r in 0..board_rows {
            circle_centers.push((
                (2 * r + c % 2) as f64 * vert_len + 0.030,
                c as f64 * vert_len + 0.030,
            ));
        }
    }
    println!("{circle_centers:?}");

    let mut imgs = vec![];
    let mut images_points: Vec<OMatrix<f64, U2, Dyn>> = vec![];
    for arg in std::env::args().skip(1) {
        let im = imread_def(&arg).unwrap();
        let cc = get_circle_centers(&im, board_rows, board_cols);
        images_points.push(
            OMatrix::<f64, U2, Dyn>::from_iterator(cc.len(), cc.into_iter().flat_map(|p| [p.x as f64, p.y as f64]))
        );
        imgs.push(im);
    }
    imshow_multi(&imgs, "Input");

    // ===== calibration =====
    let result = ccalib::calibrate(
        &circle_centers.iter().copied().map(|p| vector![p.0, p.1, 0.0]).collect::<Vec<_>>(),
        &images_points,
        radius.into(),
        6000.0,
        2047.0,
        2047.0,
        [0., 0., 0.50].into(),
    );
    let estimated_extrinsics = result.index((4.., ..)).reshape_generic(Const::<6>, Dyn((result.len()-4)/6));
    for (i, c) in estimated_extrinsics.column_iter().enumerate() {
        println!("e{i}: {:.6?}", c.as_slice());
    }
    let fx = result[0];
    let fy = result[1];
    let cx = result[2];
    let cy = result[3];
    let k = matrix![
        fx, 0., cx;
        0., fy, cy;
        0., 0., 1.;
    ];


    // ===== draw estimated extrinsics =====
    for (e, im) in estimated_extrinsics.column_iter().zip(&mut imgs) {
        let r = e.fixed_view::<3, 1>(0, 0).into_owned();
        let t = e.fixed_view::<3, 1>(3, 0).into_owned();
        let e = make_extrinsics(r, t);
        overlay_circles(im, &circle_centers, radius, k, e);
    }
    imshow_multi(&imgs, "Estimates");
}
