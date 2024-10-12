use opencv::{
    core::{no_array, Mat, Point2f, Scalar, Size, TermCriteria_EPS, TermCriteria_MAX_ITER, Vector},
    highgui::{imshow, poll_key},
    imgproc::{corner_sub_pix, cvt_color, resize, COLOR_GRAY2BGR, INTER_CUBIC},
    objdetect::{
        draw_detected_corners_charuco, draw_detected_markers, get_predefined_dictionary,
        CharucoBoard, CharucoDetector, CharucoDetectorTraitConst, CharucoParameters,
        CharucoParametersTrait, DetectorParameters, RefineParameters,
    },
};

pub fn process_image(name: &str, image: &[u8], w: i32, h: i32) {
    let im = Mat::new_rows_cols_with_data(h as i32, w as i32, image).unwrap();
    let squares_x = 2;
    let squares_y = 2;
    let square_length = 1.0;
    let marker_length = 0.8 * square_length;
    let board = CharucoBoard::new(
        Size::new(squares_x, squares_y),
        square_length,
        marker_length,
        &get_predefined_dictionary(opencv::objdetect::PredefinedDictionaryType::DICT_4X4_250)
            .unwrap(),
        &no_array(),
    )
    .unwrap();

    let detector_params = DetectorParameters::default().unwrap();
    let mut charuco_params = CharucoParameters::default().unwrap();
    charuco_params.set_try_refine_markers(true);
    // charuco_params.set_dist_coeffs([]);
    let detector = CharucoDetector::new(
        &board,
        &charuco_params,
        &detector_params,
        RefineParameters::new(10., 3., true).unwrap(),
    )
    .unwrap();

    let mut charuco_corners = Vector::<Point2f>::default();
    let mut charuco_ids = Vector::<i32>::default();
    let mut marker_corners = Vector::<Vector<Point2f>>::default();
    let mut marker_ids = Vector::<i32>::default();

    let new_size = Size::new(98, 98);
    detector
        .detect_board(
            &im,
            &mut charuco_corners,
            &mut charuco_ids,
            &mut marker_corners,
            &mut marker_ids,
        )
        .unwrap();
    if charuco_corners.len() > 0 {
        corner_sub_pix(&im, &mut charuco_corners, (5, 5).into(), (-1, -1).into(), opencv::core::TermCriteria {
            typ: TermCriteria_EPS + TermCriteria_MAX_ITER,
            max_count: 30,
            epsilon: 0.001,
        }).unwrap();
    }
    //
    // if marker_corners.len() > 0 {
    //     let mut marker_corners_flat = marker_corners.iter().flatten().collect::<Vector<Point2f>>();
    //     corner_sub_pix(&resized, &mut marker_corners_flat, (2, 2).into(), (-1, -1).into(), opencv::core::TermCriteria {
    //         typ: TermCriteria_EPS + TermCriteria_MAX_ITER,
    //         max_count: 30,
    //         epsilon: 0.001,
    //     }).unwrap();
    //     let mut sad: Vec<_> = marker_corners.iter().map(|v| v.to_vec()).collect();
    //     sad.iter_mut().flatten().zip(&marker_corners_flat).for_each(|(a, b)| *a = b);
    //     marker_corners = sad.into_iter().map(Vector::from_iter).collect();
    // }

    let mut im_copy = Mat::default();
    cvt_color(&im, &mut im_copy, COLOR_GRAY2BGR, 0).unwrap();
    let tmp = im_copy;
    let mut im_copy = Mat::default();
    resize(&tmp, &mut im_copy, Size::new(512, 512), 0., 0., INTER_CUBIC).unwrap();
    charuco_corners.as_mut_slice().iter_mut().for_each(|x| *x *= 512.0 / new_size.width as f32);
    marker_corners = marker_corners.iter().map(|mut v| {
        v.as_mut_slice().iter_mut().for_each(|x| *x *= 512.0 / new_size.height as f32);
        v
    }).collect();

    if marker_ids.len() > 0 {
        draw_detected_markers(
            &mut im_copy,
            &marker_corners,
            &no_array(),
            (0, 255, 0).into(),
        )
        .unwrap();
    }

    if charuco_ids.len() > 0 {
        draw_detected_corners_charuco(
            &mut im_copy,
            &charuco_corners,
            &charuco_ids,
            Scalar::new(255., 0., 0., 0.),
        )
        .unwrap();
    }

    imshow(name, &im_copy).unwrap();
    poll_key().unwrap();
}
