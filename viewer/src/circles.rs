use opencv::{calib3d::{ calibrate_camera, draw_chessboard_corners, find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ACCURACY, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_NORMALIZE_IMAGE }, core::{no_array, FileStorage, FileStorageTrait, FileStorageTraitConst, FileStorage_FORMAT_YAML, FileStorage_WRITE, Mat, Point2f, Point3f, Ptr, Size, TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, Vector, ROTATE_180}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{imshow, poll_key}, imgproc::{cvt_color, resize, COLOR_GRAY2BGR, INTER_CUBIC}};

use crate::Port;

/// Returns None if no circles were found.
pub fn get_circles_centers(image: &[u8; 98*98], port: Port, board_rows: i32, board_cols: i32, show: bool) -> Option<Vector<Point2f>> {
    let board_size = opencv::core::Size::new(board_rows, board_cols);
    let tmp = Mat::new_rows_cols_with_data(98, 98, image).unwrap();
    let mut im = Mat::default();
    opencv::core::flip(&tmp, &mut im, 0).unwrap();
    if port == Port::Wf {
        let tmp = im;
        im = Mat::default();
        opencv::core::rotate(&tmp, &mut im, ROTATE_180).unwrap();
    }
    let mut centers = Vector::<Point2f>::default();

    let mut params = SimpleBlobDetector_Params::default().unwrap();
    params.min_threshold = 10.0;
    params.max_threshold = 255.0;
    params.min_area = 10.0;
    params.max_area = 1500.0;
    params.filter_by_area = true;
    params.filter_by_circularity = true;
    params.min_circularity = 0.1;
    params.filter_by_inertia = true;
    params.min_inertia_ratio = 0.1;

    let mut circle_grid_finder_params = CirclesGridFinderParameters::default().unwrap();
    circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;

    let simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);

    let _pattern_was_found = find_circles_grid(
        &im,
        board_size,
        &mut centers,
        CALIB_CB_ASYMMETRIC_GRID | CALIB_CB_ACCURACY | CALIB_CB_NORMALIZE_IMAGE,
        &feature2d_detector,
        circle_grid_finder_params,
    ).unwrap();

    if show {
        display_found_circles(&im, board_size, &mut centers, port);
    }

    if centers.len() > 0 {
        Some(centers)
    } else {
        None
    }
}

fn display_found_circles(im: &Mat, board_size: opencv::core::Size, centers: &mut Vector<Point2f>, port: Port) {
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
        !centers.is_empty(),
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
    board_rows: i32,
    board_cols: i32,
) {
    let square_length = 1.0; // Specify the size of the squares between dots
    let mut board_points = Vector::<Point3f>::new();
    let mut current_row = 0f32;
    for row in 0..board_rows {
        let row_shift = if row % 2 == 0 { 0.0 } else { 0.5 };
        for col in 0..board_cols {
            board_points.push(Point3f::new(
                (col as f32 + row_shift) * square_length,
                current_row * square_length,
                0.0,
            ));
        }
        current_row += 1.0;
    }

    let corners_arr = images.iter().filter_map(|image| {
        get_circles_centers(image, port, board_rows, board_cols, false)
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
        Port::Nf => "nearfield.yml",
        Port::Wf => "widefield.yml",
    };
    let mut fs = FileStorage::new_def(filename, FileStorage_WRITE | FileStorage_FORMAT_YAML).unwrap();
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
