use std::iter::zip;

use ccalib::{make_extrinsics, utils::{distort_image, imshow_multi, stroke_circle}, CameraIntrinsics};
use nalgebra::{matrix, vector, Dyn, Matrix3, OMatrix, U2};
use opencv::{
    calib3d::{find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING}, core::{KeyPointTraitConst as _, Mat, MatTraitConst, MatTraitConstManual, MatTraitManual, Point2f, Ptr, Size, ToInputArray, Vec3b, Vector, _InputArrayTraitConst as _, CV_8UC1}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, imgcodecs::imread_def, imgproc::{resize, INTER_NEAREST}, prelude::Feature2DTrait
};

fn get_circle_centers(im: &impl ToInputArray, rows: usize, cols: usize) -> Vector<Point2f> {
    let mut centers = Vector::<Point2f>::default();

    let params = SimpleBlobDetector_Params::default().unwrap();
    let mut circle_grid_finder_params = CirclesGridFinderParameters::default().unwrap();
    circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;
    let simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
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
        // for c in centers.as_mut_slice() {
        //     *c *= 4094.0 / 97.0;
        // }
        centers
    } else {
        {
            let mut simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
            let mut keypoints = Vector::default();
            simple_blob_detector.detect_def(im, &mut keypoints).unwrap();
            let new_im = im.input_array().unwrap().get_mat_def().unwrap().clone();
            let mut new_im = {
                let mut tmp = Mat::default();
                resize(&new_im, &mut tmp, (500, 500).into(), 0.0, 0.0, INTER_NEAREST).unwrap();
                tmp
            };
            keypoints.iter().for_each(|kp| {
                let pt = (kp.pt() + (0.5, 0.5).into()) * 500.0 / 98.0 - (0.5, 0.5).into();
                let size = (kp.size() * 500.0 / 98.0) as f64;
                stroke_circle(&mut new_im, (pt.x as f64, pt.y as f64), size / 2.0, [0.0, 0.0, 255.0, 255.0], 1).unwrap();
            });
            imshow_multi(&[&new_im], "fail");
        }
        panic!("failed to find circle grid");
    }
}

fn overlay_circles(
    dst: &mut Mat,
    circle_centers: &[(f64, f64)],
    radius: f64,
    intrinsics: &CameraIntrinsics<f64>,
    e: Matrix3<f64>,
) {
    let canvas_scale = 10_000.0;
    // let output_size = Size::new(800, 800);
    let output_size = Size::new(320*3, 240*3);
    let og_size = dst.size().unwrap();
    let output_scale_x = output_size.width as f64 / og_size.width as f64;
    let output_scale_y = output_size.height as f64 / og_size.height as f64;
    let mut im = Mat::new_size_with_default((1000, 1000).into(), CV_8UC1, [0.; 4].into()).unwrap();
    for cc in circle_centers {
        let cc = (cc.0 * canvas_scale, cc.1 * canvas_scale);
        stroke_circle(&mut im, cc, radius * canvas_scale, [255.; 4], 2).unwrap();
    }

    let k = intrinsics.matrix();
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
    ];

    let mut cam = Mat::default();
    distort_image(
        &im,
        &mut cam,
        &(output_scale_mat*k),
        intrinsics.distortion.as_ref().unwrap(),
        output_size,
        &(e*canvas_scale_mat).try_inverse().unwrap(),
    );

    resize(&dst.clone(), dst, output_size, 0.0, 0.0, INTER_NEAREST).unwrap();
    let cam_data = cam.data_bytes().unwrap();
    let dst_data = dst.data_typed_mut::<Vec3b>().unwrap();
    for (cam_p, dst_p) in zip(cam_data, dst_data) {
        let c = *cam_p as f32 / 255. * 0.5;
        dst_p.0[0] = (dst_p.0[0] as f32*(1.-c)) as u8;
        dst_p.0[1] = (dst_p.0[1] as f32*(1.-c)) as u8;
        dst_p.0[2] = (dst_p.0[2] as f32*(1.-c)) as u8 + *cam_p / 2;
    }
}

fn main() {
    // ===== read images =====
    let mut circle_centers = vec![];
    let radius = 0.0045; // 9 mm
    let diag_sp = 0.012; // 12 mm
    let board_rows = 3;
    let board_cols = 5;
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
        500.0,
        160.0,
        120.0,
        [0., 0., 0.50].into(),
        true,
    );
    for (i, c) in result.extrinsics.iter().enumerate() {
        println!("e{i}: {:.6?}", c);
    }


    // ===== draw estimated extrinsics =====
    for (e, im) in result.extrinsics.iter().zip(&mut imgs) {
        let e = make_extrinsics(e.r, e.t);
        overlay_circles(im, &circle_centers, radius, &result.intrinsics, e);
    }
    imshow_multi(&imgs, "Estimates");
}
