use opencv::{calib3d::{ calibrate_camera, draw_chessboard_corners, find_circles_grid_1, stereo_calibrate, CALIB_CB_ACCURACY, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING, CALIB_CB_NORMALIZE_IMAGE, CALIB_CB_SYMMETRIC_GRID, CALIB_FIX_INTRINSIC }, core::{bitwise_and, flip, no_array, FileStorage, FileStorageTrait, FileStorageTraitConst, FileStorage_FORMAT_JSON, FileStorage_WRITE, Mat, MatExprTraitConst, MatTraitConst, Point, Point2f, Point3f, Ptr, Rect, Scalar, Size, TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, VecN, Vector, CV_32S, CV_8UC1, ROTATE_180}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{imshow, poll_key, wait_key}, imgproc::{connected_components_with_stats, cvt_color, flood_fill, flood_fill_mask, resize, threshold, COLOR_GRAY2BGR, FLOODFILL_MASK_ONLY, INTER_CUBIC, INTER_NEAREST, THRESH_BINARY, THRESH_BINARY_INV}, traits::Boxed};

use crate::{chessboard::read_camara_params, Port, CALIBRATION_VERSION};

fn find_circles_grid_special(
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
    let threshold_distance = 20.0 * 4095.0 / 97.0;

    while sorted_hull_points.len() < expected_num_outer_points {
        let mut best_point = None;
        let mut best_distance = f32::MAX;

        for point in centers.iter() {
            let distance = (point - current).norm();
            if distance < threshold_distance && distance > 0.0 {
                let direction = point - current;
                let dot_product = direction.dot(directions[current_direction]);

                if dot_product > 0.9 && distance < best_distance.into() {
                    best_distance = distance as f32;
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

// Helper function to calculate the perpendicular distance from a point to a line
// fn point_line_distance(p: &Point2f, a: &Point2f, b: &Point2f) -> f32 {
//     let numerator = ((b.y - a.y) * p.x - (b.x - a.x) * p.y + b.x * a.y - b.y * a.x).abs();
//     let denominator = ((b.y - a.y).powi(2) + (b.x - a.x).powi(2)).sqrt();
//     numerator / denominator
// }

/// Returns None if no circles were found. invert = false -> detect dark blobs, invert = true ->
/// detect white blobs
pub fn get_circles_centers(image: &[u8; 98*98], port: Port, board_rows: u16, board_cols: u16, show: bool, upside_down: bool, asymmetric: bool, invert: bool, special: bool) -> Option<Vector<Point2f>> {
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

    let im2 = im.clone();

    let mut thresholded = Mat::default();
    threshold(
        &im2,
        &mut thresholded,
        110. - 15.,
        255.,
        if invert { THRESH_BINARY } else { THRESH_BINARY_INV }
    ).unwrap();

    // Label the initial seed regions (connected components in the thresholded image)
    let mut seed_labels = Mat::default();
    let mut seed_stats = Mat::default();
    let mut seed_centroids = Mat::default();

    let num_seed_labels = connected_components_with_stats(
        &thresholded,
        &mut seed_labels,
        &mut seed_stats,
        &mut seed_centroids,
        8,
        CV_32S
    ).unwrap();

    let mut centers = Vector::<Point2f>::default();

    // Now use these seeds to perform a flood fill or extended connected components
    for label in 1..num_seed_labels {
        // area
        let stats = *seed_stats.at_2d::<i32>(label, 4).unwrap();
        if stats < 3 {
            continue;
        }

        let mut mask = Mat::default();

        // Create a binary mask for the current seed label
        opencv::core::in_range(
            &seed_labels,
            &Scalar::from(label as f64),
            &Scalar::from(label as f64),
            &mut mask
        ).unwrap();

        // Mask the original image to consider only the region of the current blob
        let mut masked_region = Mat::default();
        bitwise_and(&im2, &im2, &mut masked_region, &mask).unwrap();

        // imshow("img2", &im2).unwrap();

        // let mut im2_scaled = Mat::default();
        // resize(&im2, &mut im2_scaled, Size::new(512, 512), 0.0, 0.0, INTER_NEAREST).unwrap();
        // imshow("img2", &im2_scaled).unwrap();

        // let mut masked_region_scaled = Mat::default();
        // resize(&masked_region, &mut masked_region_scaled, Size::new(512, 512), 0.0, 0.0, INTER_NEAREST).unwrap();
        // imshow("masked_region", &masked_region_scaled).unwrap();
        // wait_key(0).unwrap();

        // Calculate moments on the masked region to get the centroid
        let moments = opencv::imgproc::moments(&masked_region, false).unwrap();
        if moments.m00 > 0. {
            let center = Point2f::new(
                (moments.m10 / moments.m00) as f32 * 4095.0 / 97.0,
                (moments.m01 / moments.m00) as f32 * 4095.0 / 97.0,
            );
            centers.push(center);
        }
    }

    if show {
        display_found_circles(&im, board_size, &centers, true, port);
    }

    if !special {
        if centers.len() != (board_rows * board_cols) as usize {
            return None;
        }
    }

    let mut sorted_centers = Vector::<Point2f>::default();

    if special {
        if let Some(_centers) = find_circles_grid_special(&centers, board_size) {
            if _centers.len() != (board_rows*2 + board_cols*2 - 4) as usize {
                return None;
            }
            sorted_centers = _centers;
        }
    } else {
        let nullf2d;
        unsafe {
            nullf2d = Feature2D::from_raw(core::ptr::null_mut());
        }
        find_circles_grid_1(&centers, board_size, &mut sorted_centers, CALIB_CB_SYMMETRIC_GRID, &Ptr::new(nullf2d)).unwrap();
    }

    if !sorted_centers.is_empty() {
        Some(sorted_centers)
    } else {
        None
    }
}

fn display_found_circles(im: &Mat, board_size: opencv::core::Size, centers: &Vector<Point2f>, pattern_was_found: bool, port: Port) {
    let mut display_im = Mat::default();
    cvt_color(&im, &mut display_im, COLOR_GRAY2BGR, 0).unwrap();
    let tmp = display_im;
    let mut display_im = Mat::default();
    resize(&tmp, &mut display_im, Size::new(512, 512), 0.0, 0.0, INTER_CUBIC).unwrap();

    let mut centers = centers.clone();
    centers.as_mut_slice().iter_mut().for_each(|x| {
        *x += (0.5, 0.5).into();
        *x *= 512.0 / 4095. as f32;
        let half_pixel = (512.0 / 4095. as f32) / 2.;
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
    images: &[[u8; 98 * 98]],
    port: Port,
    board_rows: u16,
    board_cols: u16,
    upside_down: bool,
    asymmetric: bool,
    invert: bool,
    special: bool,
) {
    let square_length = 1.0; // Specify the size of the squares between dots
    let board_points = board_points(board_rows, board_cols, square_length, asymmetric, special);

    let corners_arr = images.iter().filter_map(|image| {
        get_circles_centers(image, port, board_rows, board_cols, false, upside_down, asymmetric, invert, special)
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
        (4096, 4096).into(),
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
        fs.write_i32("version", CALIBRATION_VERSION).unwrap();
        fs.release().unwrap();
    } else {
        println!("Failed to open {}", filename);
    }
}

fn board_points(
    board_rows: u16,
    board_cols: u16,
    square_length: f32,
    asymmetric: bool,
    special: bool
) -> Vector<opencv::core::Point3_<f32>> {
    let mut board_points = Vector::<Point3f>::new();

    if special {
        // Assuming the pattern is the outer boundary of the grid, in clockwise order
        for col in 0..board_cols {
            board_points.push(Point3f::new(
                col as f32 * square_length,
                0.0,
                0.0,
            ));
        }
        for row in 1..board_rows {
            board_points.push(Point3f::new(
                (board_cols - 1) as f32 * square_length,
                row as f32 * square_length,
                0.0,
            ));
        }
        for col in (0..board_cols - 1).rev() {
            board_points.push(Point3f::new(
                col as f32 * square_length,
                (board_rows - 1) as f32 * square_length,
                0.0,
            ));
        }
        for row in (1..board_rows - 1).rev() {
            board_points.push(Point3f::new(
                0.0,
                row as f32 * square_length,
                0.0,
            ));
        }
    } else if asymmetric {
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
                board_points.push(Point3f::new(
                    col as f32 * square_length,
                    row as f32 * square_length,
                    0.0,
                ));
            }
        }
    }

    board_points
}

pub fn my_stereo_calibrate(
    wf: &[[u8; 98 * 98]],
    nf: &[[u8; 98 * 98]],
    board_rows: u16,
    board_cols: u16,
    upside_down: bool,
    asymmetric: bool,
    invert: bool,
    special: bool,
) {
    let Some((nf_camera_matrix, nf_dist_coeffs)) = &mut read_camara_params("nearfield.json") else {
        println!("Couldn't get nearfield intrinsics");
        return;
    };
    let Some((wf_camera_matrix, wf_dist_coeffs)) = &mut read_camara_params("widefield.json") else {
        println!("Couldn't get widefield intrinsics");
        return;
    };

    let square_length = 1.26; // 7x5 circles grid that i printed out
    let object_points = board_points(board_rows, board_cols, square_length, asymmetric, special);

    let mut object_points_arr = Vector::<Vector<Point3f>>::new();
    let mut wf_points_arr = Vector::<Vector<Point2f>>::new();
    let mut nf_points_arr = Vector::<Vector<Point2f>>::new();
    for (wf_image, nf_image) in wf.iter().zip(nf) {
        let Some(wf_points) =
            get_circles_centers(wf_image, Port::Wf, board_rows, board_cols, false, upside_down, asymmetric, invert, special)
        else {
            continue;
        };
        let Some(nf_points) =
            get_circles_centers(nf_image, Port::Nf, board_rows, board_cols, false, upside_down, asymmetric, invert, special)
        else {
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
    let reproj_err = stereo_calibrate(
        &object_points_arr,
        &wf_points_arr,
        &nf_points_arr,
        wf_camera_matrix,
        wf_dist_coeffs,
        nf_camera_matrix,
        nf_dist_coeffs,
        (4096, 4096).into(),
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
