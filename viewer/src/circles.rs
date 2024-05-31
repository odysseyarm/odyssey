use opencv::{calib3d::{ calibrate_camera, draw_chessboard_corners, find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ACCURACY, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_NORMALIZE_IMAGE }, core::{flip, no_array, FileStorage, FileStorageTrait, FileStorageTraitConst, FileStorage_FORMAT_JSON, FileStorage_WRITE, Mat, Point2f, Point3f, Ptr, Size, TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, Vector, ROTATE_180}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{imshow, poll_key}, imgproc::{cvt_color, resize, COLOR_GRAY2BGR, INTER_CUBIC}};

use crate::Port;

/// Returns None if no circles were found.
pub fn get_circles_centers(image: &[u8; 98*98], port: Port, board_rows: u16, board_cols: u16, show: bool, upside_down: bool) -> Option<Vector<Point2f>> {
    let board_size = opencv::core::Size::new(board_cols as i32, board_rows as i32);
    let tmp = Mat::new_rows_cols_with_data(98, 98, image).unwrap();
    let mut im = Mat::default();
    flip(&tmp, &mut im, 0).unwrap();
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
    let mut im2 = Mat::default();
    resize(&im, &mut im2, Size::new(512, 512), 0.0, 0.0, INTER_CUBIC).unwrap();
    let mut centers = Vector::<Point2f>::default();

    let mut params = SimpleBlobDetector_Params::default().unwrap();
    params.min_threshold = 0.0;
    params.max_threshold = 255.0;
    params.min_area = 50.0;
    params.max_area = 10000.0;
    params.filter_by_area = true;
    params.filter_by_convexity = true;
    params.min_convexity = 0.87;
    params.min_circularity = 0.1;
    params.filter_by_circularity = true;
    params.min_inertia_ratio = 0.01;
    params.filter_by_inertia = true;

    let mut circle_grid_finder_params = CirclesGridFinderParameters::default().unwrap();
    circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;

    let simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);

    let pattern_was_found = find_circles_grid(
        &im2,
        board_size,
        &mut centers,
        CALIB_CB_ASYMMETRIC_GRID,
        &feature2d_detector,
        circle_grid_finder_params,
    ).unwrap();

    centers.as_mut_slice().iter_mut().for_each(|x| {
        *x *= 98.0 / 512. as f32;
    });

    if show {
        display_found_circles(&im, board_size, &mut centers, pattern_was_found, port);
    }

    if pattern_was_found {
        Some(centers)
    } else {
        None
    }
}

fn display_found_circles(im: &Mat, board_size: opencv::core::Size, centers: &mut Vector<Point2f>, pattern_was_found: bool, port: Port) {
    let mut display_im = Mat::default();
    cvt_color(&im, &mut display_im, COLOR_GRAY2BGR, 0).unwrap();
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
        centers,
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
    images: &[[u8; 98 * 98]],
    port: Port,
    board_rows: u16,
    board_cols: u16,
    upside_down: bool,
) {
    let square_length = 1.0; // Specify the size of the squares between dots
    let mut board_points = Vector::<Point3f>::new();
    for row in 0..board_rows {
        for col in 0..board_cols {
            board_points.push(Point3f::new(
                (2*col + row%2) as f32 * square_length,
                row as f32 * square_length,
                0.0,
            ));
        }
    }

    let corners_arr = images.iter().filter_map(|image| {
        get_circles_centers(image, port, board_rows, board_cols, false, upside_down)
    }).collect::<Vector<Vector<Point2f>>>();
    let object_points: Vector<Vector<Point3f>> = std::iter::repeat(board_points).take(corners_arr.len()).collect();

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
        (98, 98).into(),
        &mut camera_matrix,
        &mut dist_coeffs,
        &mut no_array(),
        &mut no_array(),
        0,
        criteria
    ).unwrap();
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
