use nalgebra::{Dyn, OMatrix, RowVector5, Vector3, U2};
use opencv::{calib3d::{draw_chessboard_corners, find_circles_grid, CirclesGridFinderParameters, CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING, CALIB_CB_SYMMETRIC_GRID }, core::{no_array, Mat, Point2f, Point3f, Ptr, Size, TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, ToInputArray, Vector, _InputArrayTraitConst as _}, features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params}, highgui::{imshow, poll_key}, imgproc::{cvt_color_def, resize, COLOR_GRAY2BGR, INTER_CUBIC}};

use crate::{chessboard::read_camera_params, utils::nalg_to_mat, DeviceUuid, PatternPoints, Port};

pub mod special;

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
    let board_size = opencv::core::Size::new(board_rows as i32, board_cols as i32);

    let im = Mat::new_rows_cols_with_data(resolution.1.into(), resolution.0.into(), image).unwrap();

    let params = SimpleBlobDetector_Params::default().unwrap();
    let mut circle_grid_finder_params = CirclesGridFinderParameters::default().unwrap();
    if asymmetric {
        circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;
    } else {
        circle_grid_finder_params.grid_type = opencv::calib3d::CirclesGridFinderParameters_GridType::SYMMETRIC_GRID;
    }

    let simple_blob_detector = SimpleBlobDetector::create(params).unwrap();
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);
    let mut centers = Vector::<Point2f>::default();

    let pattern_found = find_circles_grid(
        &im,
        board_size,
        &mut centers,
        if asymmetric { CALIB_CB_ASYMMETRIC_GRID } else { CALIB_CB_SYMMETRIC_GRID } | CALIB_CB_CLUSTERING,
        Some(&feature2d_detector),
        circle_grid_finder_params,
    ).unwrap();

    if show {
        display_found_circles(&im, board_size, &mut centers, pattern_found, port);
    }

    let size = im.input_array().unwrap().size(-1).unwrap();
    for center in centers.as_mut_slice() {
        center.x *= (object_resolution.0 - 1) as f32 / (size.width - 1) as f32;
        center.y *= (object_resolution.1 - 1) as f32 / (size.height - 1) as f32;
    }
    PatternPoints {
        points: centers,
        pattern_found,
    }
}

fn display_found_circles(im: &impl ToInputArray, board_size: opencv::core::Size, centers: &Vector<Point2f>, pattern_was_found: bool, port: Port) {
    let mut centers = centers.clone();
    let mut display_im = Mat::default();
    cvt_color_def(im, &mut display_im, COLOR_GRAY2BGR).unwrap();
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
    spacing_mm: u16,
    diameter_mm: u16,
    asymmetric: bool,
    device_uuid: DeviceUuid,
) {
    let spacing = spacing_mm as f32 / 1000.0;
    let radius = diameter_mm as f64 / 1000.0 / 2.0;
    let board_points = board_points(board_rows, board_cols, spacing, asymmetric)
        .into_iter()
        .map(|p| Vector3::new(p.x, p.y, p.z).cast::<f64>())
        .collect::<Vec<_>>();

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
            if r.pattern_found {
                let pts = OMatrix::<f64, U2, Dyn>::from_iterator(
                    r.points.len(),
                    r.points.into_iter().flat_map(|p| [p.x as f64, p.y as f64]),
                );
                Some(pts)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let initial_focal_length = if port == Port::Nf {
        6000.0
    } else if resolution == (98, 98) {
        1500.0
    } else if resolution == (320, 240) {
        13_000.0
    } else {
        println!("Missing initial guess for focal length!");
        object_resolution.0.min(object_resolution.1) as f64
    };
    let mut flags = ccalib::Flags::empty();
    flags.set(ccalib::Flags::RADIAL_DISTORTION, port == Port::Wf);
    let result = ccalib::calibrate(
        &board_points,
        &corners_arr,
        radius,
        initial_focal_length,
        object_resolution.0 as f64 / 2.0,
        object_resolution.1 as f64 / 2.0,
        Vector3::new(0.0, 0.0, radius * 10.0),
        flags,
    );
    println!("RMS error: {}", result.rmse);

    let pattern = match asymmetric {
        true => "acircles",
        false => "circles",
    };
    super::save_single_calibration(
        port,
        pattern,
        &nalg_to_mat(&result.intrinsics.matrix()),
        &nalg_to_mat(&RowVector5::from(result.intrinsics.distortion())),
        result.rmse,
        image_files,
        device_uuid,
    );
}

fn board_points(
    board_rows: u16,
    board_cols: u16,
    spacing: f32,
    asymmetric: bool,
) -> Vector<opencv::core::Point3_<f32>> {
    let mut board_points = Vector::<Point3f>::new();

    if asymmetric {
        // spacing is diagonal spacing for asymmetric grids
        let diag_sp = spacing;
        let spacing = (2.0 * diag_sp * diag_sp).sqrt() / 2.0;
        for col in 0..board_cols {
            for row in 0..board_rows {
                board_points.push(Point3f::new(
                    (2 * row + col % 2) as f32 * spacing,
                    col as f32 * spacing,
                    0.0,
                ));
            }
        }
    } else {
        for row in 0..board_rows {
            for col in 0..board_cols {
                board_points.push(Point3f::new(col as f32 * spacing, row as f32 * spacing, 0.0));
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
