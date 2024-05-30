use opencv::{
    calib3d::{calibrate_camera, draw_chessboard_corners, find_chessboard_corners, find_chessboard_corners_sb, stereo_calibrate, CALIB_CB_ACCURACY, CALIB_CB_NORMALIZE_IMAGE, CALIB_FIX_INTRINSIC}, core::{flip, no_array, FileStorage, FileStorage_FORMAT_JSON, FileStorage_READ, FileStorage_WRITE, Mat, Point2f, Point3f, Size, TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, TermCriteria_MAX_ITER, ToInputArray, Vector, ROTATE_180}, highgui::{imshow, poll_key}, hub_prelude::{FileNodeTraitConst, FileStorageTrait, FileStorageTraitConst}, imgproc::{corner_sub_pix, cvt_color, resize, COLOR_GRAY2BGR, INTER_CUBIC}
};

use crate::Port;

pub fn read_camara_params(path: &str) -> Option<(Mat, Mat)> {
    let fs = FileStorage::new_def(path, FileStorage_READ).unwrap();
    if !fs.is_opened().unwrap() {
        None
    } else {
        let camera_matrix = fs.get("camera_matrix").unwrap().mat().unwrap();
        let dist_coeffs = fs.get("dist_coeffs").unwrap().mat().unwrap();
        Some((camera_matrix, dist_coeffs))
    }
}

pub fn my_stereo_calibrate(
    wf: &[[u8; 98 * 98]],
    nf: &[[u8; 98 * 98]],
    board_rows: u16,
    board_cols: u16,
    upside_down: bool,
) {
    let Some((nf_camera_matrix, nf_dist_coeffs)) = &mut read_camara_params("nearfield.json") else {
        println!("Couldn't get nearfield intrinsics");
        return;
    };
    let Some((wf_camera_matrix, wf_dist_coeffs)) = &mut read_camara_params("widefield.json") else {
        println!("Couldn't get widefield intrinsics");
        return;
    };

    let square_length = 1.0; // 4x4 chessboard on my phone is barely above 1 cm
    let mut object_points = Vector::<Point3f>::new();
    for y in 0..board_rows {
        for x in 0..board_cols {
            object_points.push(Point3f::new(x as f32, y as f32, 0.0) * square_length);
        }
    }
    let object_points = object_points;

    let mut object_points_arr = Vector::<Vector<Point3f>>::new();
    let mut wf_corners_arr = Vector::<Vector<Point2f>>::new();
    let mut nf_corners_arr = Vector::<Vector<Point2f>>::new();
    for (wf_image, nf_image) in wf.iter().zip(nf) {
        let Some(wf_corners) =
            get_chessboard_corners(wf_image, Port::Wf, board_rows, board_cols, false, upside_down)
        else {
            continue;
        };
        let Some(nf_corners) =
            get_chessboard_corners(nf_image, Port::Nf, board_rows, board_cols, false, upside_down)
        else {
            continue;
        };
        wf_corners_arr.push(wf_corners);
        nf_corners_arr.push(nf_corners);
        object_points_arr.push(object_points.clone());
    }
    let mut r = Mat::default();
    let mut t = Mat::default();
    let criteria = TermCriteria {
        typ: TermCriteria_EPS + TermCriteria_COUNT,
        max_count: 30,
        epsilon: 1e-6,
    };
    let reproj_err = stereo_calibrate(
        &object_points_arr,
        &wf_corners_arr,
        &nf_corners_arr,
        wf_camera_matrix,
        wf_dist_coeffs,
        nf_camera_matrix,
        nf_dist_coeffs,
        (98, 98).into(),
        &mut r,
        &mut t,
        &mut no_array(),
        &mut no_array(),
        CALIB_FIX_INTRINSIC,
        criteria,
    )
    .unwrap();
    println!("RMS error: {}", reproj_err);

    let mut fs = FileStorage::new_def("stereo.json", FileStorage_WRITE | FileStorage_FORMAT_JSON).unwrap();
    if fs.is_opened().unwrap() {
        fs.write_mat("r", &r).unwrap();
        fs.write_mat("t", &t).unwrap();
        fs.write_f64("rms_error", reproj_err).unwrap();
        fs.write_i32("num_captures", wf.len() as i32).unwrap();
    } else {
        println!("Failed to open stereo.json");
    }
}

pub fn calibrate_single(
    images: &[[u8; 98 * 98]],
    port: Port,
    board_rows: u16,
    board_cols: u16,
    upside_down: bool,
) {
    let mut board_points = Vector::<Point3f>::new();
    for y in 0..board_rows {
        for x in 0..board_cols {
            board_points.push(Point3f::new(x as f32, y as f32, 0.0));
        }
    }

    let corners_arr = images.iter().filter_map(|image| {
        get_chessboard_corners(image, port, board_rows, board_cols, false, upside_down)
    }).collect::<Vector<Vector<Point2f>>>();
    let object_points: Vector<Vector<Point3f>> = std::iter::repeat(board_points).take(corners_arr.len()).collect();

    let mut camera_matrix = Mat::default();
    let mut dist_coeffs = Mat::default();
    let criteria = TermCriteria {
        typ: TermCriteria_EPS + TermCriteria_COUNT,
        max_count: 30,
        epsilon: f64::EPSILON,
    };
    let reproj_err = calibrate_camera(&object_points, &corners_arr, (98, 98).into(), &mut camera_matrix, &mut dist_coeffs, &mut no_array(), &mut no_array(), 0, criteria).unwrap();
    println!("RMS error: {}", reproj_err);

    let filename = match port {
        Port::Nf => "nearfield.json",
        Port::Wf => "widefield.json",
    };
    let mut fs = FileStorage::new_def(filename, FileStorage_WRITE | FileStorage_FORMAT_JSON).unwrap();
    if fs.is_opened().unwrap() {
        fs.write_mat("camera_matrix", &camera_matrix).unwrap();
        fs.write_mat("dist_coeffs", &dist_coeffs).unwrap();
        fs.write_f64("rms_error", reproj_err).unwrap();
        fs.write_i32("num_captures", images.len() as i32).unwrap();
        fs.release().unwrap();
    } else {
        println!("Failed to open {}", filename);
    }
}

/// Returns None if there were no corners found.
pub fn get_chessboard_corners(image: &[u8; 98*98], port: Port, board_rows: u16, board_cols: u16, show: bool, upside_down: bool) -> Option<Vector<Point2f>> {
    let tmp = Mat::new_rows_cols_with_data(98, 98, image).unwrap();
    let mut im = Mat::default();
    if !upside_down {
        flip(&tmp, &mut im, 0).unwrap();
        if port == Port::Wf {
            let tmp = im;
            im = Mat::default();
            opencv::core::rotate(&tmp, &mut im, ROTATE_180).unwrap();
        }
    } else {
        flip(&tmp, &mut im, 0).unwrap();
        if port == Port::Nf {
            let tmp = im;
            im = Mat::default();
            opencv::core::rotate(&tmp, &mut im, ROTATE_180).unwrap();
        }
    }
    get_chessboard_corners_cv(&im, port, board_rows, board_cols, show)
}

pub fn get_chessboard_corners_cv(image: &impl ToInputArray, port: Port, board_rows: u16, board_cols: u16, show: bool) -> Option<Vector<Point2f>> {
    let board_rows = i32::from(board_rows);
    let board_cols = i32::from(board_cols);

    let mut corners = Vector::<Point2f>::default();
    let use_sb = true;
    let mut im_upscaled = Mat::default();
    // super resolution scale
    let ss = 4.;
    resize(image, &mut im_upscaled, Size::new(0, 0), ss, ss, INTER_CUBIC).unwrap();
    if use_sb {
        let _chessboard_found = find_chessboard_corners_sb(&im_upscaled, (board_cols, board_rows).into(), &mut corners, 0).unwrap();
    } else {
        let _chessboard_found = find_chessboard_corners(&im_upscaled, (board_cols, board_rows).into(), &mut corners, 0).unwrap();
        if corners.len() > 0 {
            corner_sub_pix(&im_upscaled, &mut corners, (3, 3).into(), (-1, -1).into(), opencv::core::TermCriteria {
                typ: TermCriteria_EPS + TermCriteria_MAX_ITER,
                max_count: 30,
                epsilon: 0.001,
            }).unwrap();
        }
    }
    // scale corners back down
    corners.as_mut_slice().iter_mut().for_each(|x| {
        *x /= ss as f32;
    });
    // let chessboard_found = find_chessboard_corners_sb_with_meta(
    //     &im,
    //     (board_cols, board_rows).into(),
    //     &mut corners,
    //     CALIB_CB_MARKER + CALIB_CB_LARGER,
    //     &mut meta,
    // )
    // .unwrap();

    if show {
        let mut im_copy = Mat::default();
        cvt_color(image, &mut im_copy, COLOR_GRAY2BGR, 0).unwrap();
        let tmp = im_copy;
        let mut im_copy = Mat::default();
        resize(&tmp, &mut im_copy, Size::new(512, 512), 0., 0., INTER_CUBIC).unwrap();
        corners.as_mut_slice().iter_mut().for_each(|x| {
            *x *= 512.0 / 98. as f32;
        });

        if corners.len() > 0 {
            draw_chessboard_corners(
                &mut im_copy,
                (board_cols, board_rows).into(),
                &corners,
                !corners.is_empty(),
            )
            .unwrap();
        }

        let name = match port {
            Port::Wf => "wf",
            Port::Nf => "nf",
        };
        imshow(name, &im_copy).unwrap();
        poll_key().unwrap();
    }
    if corners.len() > 0 {
        Some(corners)
    } else {
        None
    }
}
