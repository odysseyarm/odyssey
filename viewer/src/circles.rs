use opencv::{calib3d::{ calibrate_camera, draw_chessboard_corners, find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING, CALIB_CB_SYMMETRIC_GRID }, core::{no_array, Mat, MatTraitConst as _, Point2f, Point3f, Ptr, Size, TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, ToInputArray as _, Vector, _InputArrayTraitConst as _}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{imshow, poll_key}, imgproc::{cvt_color_def, resize, COLOR_GRAY2BGR, INTER_CUBIC}};

use crate::{chessboard::read_camera_params, DeviceUuid, PatternPoints, Port};

/// Returns None if no circles were found.
pub fn get_circles_centers(
    image: &[u8],
    port: Port,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    board_rows: u16,
    board_cols: u16,
    asymmetric: bool,
    show: bool,
) -> PatternPoints {
    let board_size = opencv::core::Size::new(board_cols as i32, board_rows as i32);

    let im = Mat::new_rows_cols_with_data(resolution.1.into(), resolution.0.into(), image).unwrap();

    let mut im_upscaled = Mat::default();
    // super resolution scale
    let ss;
    if resolution == (98, 98) {
        ss = 4.;
        resize(&im, &mut im_upscaled, Size::new(0, 0), ss, ss, INTER_CUBIC).unwrap();
    } else {
        ss = 1.;
        // resize(&im, &mut im_upscaled, Size::new(0, 0), ss, ss, INTER_CUBIC).unwrap();
        im_upscaled = im.try_clone().unwrap();
    }

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
    if asymmetric {
        circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;
    } else {
        circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::SYMMETRIC_GRID;
    }

    let simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);

    let pattern_was_found = find_circles_grid(
        &im_upscaled,
        board_size,
        &mut centers,
        if asymmetric { CALIB_CB_ASYMMETRIC_GRID } else { CALIB_CB_SYMMETRIC_GRID } | CALIB_CB_CLUSTERING,
        &feature2d_detector,
        circle_grid_finder_params,
    ).unwrap();

    centers.as_mut_slice().iter_mut().for_each(|x| {
        x.x = (x.x + 0.5) / ss as f32 - 0.5;
        x.y = (x.y + 0.5) / ss as f32 - 0.5;
    });

    if show {
        display_found_circles(&im_upscaled, board_size, &mut centers, pattern_was_found, port);
    }

    let size = im.input_array().unwrap().size(-1).unwrap();
    for center in centers.as_mut_slice() {
        center.x *= (object_resolution.0 - 1) as f32 / (size.width - 1) as f32;
        center.y *= (object_resolution.1 - 1) as f32 / (size.height - 1) as f32;
    }
    PatternPoints {
        points: centers,
        pattern_found: pattern_was_found,
    }
}

fn display_found_circles(im: &Mat, board_size: opencv::core::Size, centers: &Vector<Point2f>, pattern_was_found: bool, port: Port) {
    let mut centers = centers.clone();
    let mut display_im = Mat::default();
    cvt_color_def(&im, &mut display_im, COLOR_GRAY2BGR).unwrap();
    let tmp = display_im;
    let mut display_im = Mat::default();
    resize(&tmp, &mut display_im, Size::new(512, 512), 0.0, 0.0, INTER_CUBIC).unwrap();
    centers.as_mut_slice().iter_mut().for_each(|x| {
        *x *= 512.0 / 98. as f32;
        let half_pixel = (512.0 / 98. as f32) / 2.;
        *x += (half_pixel, half_pixel).into();
    });

    draw_chessboard_corners(
        &mut display_im,
        board_size,
        &centers,
        pattern_was_found,
    ).unwrap();

    let name = match port {
        Port::Wf => "wf",
        Port::Nf => "nf",
    };
    imshow(name, &display_im).unwrap();
    poll_key().unwrap();
}

pub fn calibrate_single(
    images: &[Vec<u8>],
    image_files: &Vector<String>,
    port: Port,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    board_rows: u16,
    board_cols: u16,
    asymmetric: bool,
    device_uuid: DeviceUuid,
) {
    let square_length = 1.0; // Specify the size of the squares between dots.
    let board_points = board_points(board_rows, board_cols, square_length, asymmetric);

    let corners_arr = images
        .iter()
        .filter_map(|image| {
            let r = get_circles_centers(
                image,
                port,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                asymmetric,
                false,
            );
            r.pattern_found.then_some(r.points)
        })
        .collect::<Vector<Vector<Point2f>>>();
    let object_points: Vector<Vector<Point3f>> =
        std::iter::repeat(board_points).take(corners_arr.len()).collect();

    let mut camera_matrix = Mat::default();
    let mut dist_coeffs = Mat::default();
    let criteria = TermCriteria {
        typ: TermCriteria_EPS + TermCriteria_COUNT,
        max_count: 30,
        epsilon: f64::EPSILON,
    };
    let reproj_err = calibrate_camera(
        &object_points,
        &corners_arr,
        (object_resolution.0 as i32, object_resolution.1 as i32).into(),
        &mut camera_matrix,
        &mut dist_coeffs,
        &mut no_array(),
        &mut no_array(),
        0,
        criteria,
    )
    .unwrap();
    println!("RMS error: {}", reproj_err);

    let pattern = match asymmetric {
        true => "acircles",
        false => "circles",
    };
    super::save_single_calibration(
        port,
        pattern,
        &camera_matrix,
        &dist_coeffs,
        reproj_err,
        image_files,
        device_uuid,
    );
}

fn board_points(
    board_rows: u16,
    board_cols: u16,
    square_length: f32,
    asymmetric: bool,
) -> Vector<opencv::core::Point3_<f32>> {
    let mut board_points = Vector::<Point3f>::new();

    if asymmetric {
        for row in 0..board_rows {
            for col in 0..board_cols {
                board_points.push(Point3f::new(
                    (2 * col + row % 2) as f32 * square_length,
                    row as f32 * square_length,
                    0.0,
                ));
            }
        }
    } else {
        for row in 0..board_rows {
            for col in 0..board_cols {
                board_points.push(Point3f::new(col as f32 * square_length, row as f32 * square_length, 0.0));
            }
        }
    }

    board_points
}

pub fn my_stereo_calibrate(
    wf: &[Vec<u8>],
    nf: &[Vec<u8>],
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    board_rows: u16,
    board_cols: u16,
    asymmetric: bool,
    wf_image_files: &Vector<String>,
    nf_image_files: &Vector<String>,
    device_uuid: DeviceUuid,
) {
    let Some((nf_camera_matrix, nf_dist_coeffs)) = &mut read_camera_params(&format!("calibrations/{device_uuid}/nearfield.json")) else {
        println!("Couldn't get nearfield intrinsics");
        return;
    };
    let Some((wf_camera_matrix, wf_dist_coeffs)) = &mut read_camera_params(&format!("calibrations/{device_uuid}/widefield.json")) else {
        println!("Couldn't get widefield intrinsics");
        return;
    };

    let square_length = 1.26; // For example, a 7x5 circles grid that was printed out.
    let object_points = board_points(board_rows, board_cols, square_length, asymmetric);

    let mut object_points_arr = Vector::<Vector<Point3f>>::new();
    let mut wf_points_arr = Vector::<Vector<Point2f>>::new();
    let mut nf_points_arr = Vector::<Vector<Point2f>>::new();
    for (wf_image, nf_image) in wf.iter().zip(nf) {
        let PatternPoints { points: wf_points, pattern_found: true } = get_circles_centers(
            wf_image,
            Port::Wf,
            resolution,
            object_resolution,
            board_rows,
            board_cols,
            false,
            asymmetric,
        ) else {
            continue;
        };
        let PatternPoints { points: nf_points, pattern_found: true } = get_circles_centers(
            nf_image,
            Port::Nf,
            resolution,
            object_resolution,
            board_rows,
            board_cols,
            false,
            asymmetric,
        ) else {
            continue;
        };
        wf_points_arr.push(wf_points);
        nf_points_arr.push(nf_points);
        object_points_arr.push(object_points.clone());
    }
    let mut r = Mat::default();
    let mut t = Mat::default();
    let criteria = TermCriteria {
        typ: TermCriteria_EPS + TermCriteria_COUNT,
        max_count: 30,
        epsilon: 1e-6,
    };
    let reproj_err = opencv::calib3d::stereo_calibrate(
        &object_points_arr,
        &wf_points_arr,
        &nf_points_arr,
        wf_camera_matrix,
        wf_dist_coeffs,
        nf_camera_matrix,
        nf_dist_coeffs,
        (object_resolution.0 as i32, object_resolution.1 as i32).into(),
        &mut r,
        &mut t,
        &mut no_array(),
        &mut no_array(),
        opencv::calib3d::CALIB_FIX_INTRINSIC,
        criteria,
    )
    .unwrap();
    println!("RMS error: {}", reproj_err);

    let pattern = match asymmetric {
        false => "circles",
        true => "acircles",
    };
    super::save_stereo_calibration(
        pattern,
        r,
        t,
        reproj_err,
        wf_image_files,
        nf_image_files,
        device_uuid,
    );
}
