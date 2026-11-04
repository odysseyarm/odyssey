use opencv::calib3d::{
    CALIB_CB_ASYMMETRIC_GRID, CALIB_CB_CLUSTERING, CALIB_CB_SYMMETRIC_GRID,
    CirclesGridFinderParameters, draw_chessboard_corners, find_circles_grid,
};
use opencv::core::{
    _InputArrayTraitConst as _, Mat, Point2f, Point3f, Ptr, Size, ToInputArray, Vector,
};
use opencv::features2d::{Feature2D, SimpleBlobDetector, SimpleBlobDetector_Params};
use opencv::highgui::{imshow, poll_key};
use opencv::imgproc::{COLOR_GRAY2BGR, INTER_CUBIC, cvt_color_def, resize};

use crate::{PatternPoints, Port, ViewerError};

pub mod special;

/// Returns detected points and pattern status, or the OpenCV failure.
pub fn get_circles_centers(
    image: &[u8],
    port: Port,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    board_rows: u16,
    board_cols: u16,
    asymmetric: bool,
    show: bool,
) -> Result<PatternPoints, ViewerError> {
    let board_size = opencv::core::Size::new(board_cols as i32, board_rows as i32);

    let im = Mat::new_rows_cols_with_data(resolution.1.into(), resolution.0.into(), image)?;

    let params = SimpleBlobDetector_Params::default()?;
    let mut circle_grid_finder_params = CirclesGridFinderParameters::default()?;
    if asymmetric {
        circle_grid_finder_params.grid_type =
            opencv::calib3d::CirclesGridFinderParameters_GridType::ASYMMETRIC_GRID;
    } else {
        circle_grid_finder_params.grid_type =
            opencv::calib3d::CirclesGridFinderParameters_GridType::SYMMETRIC_GRID;
    }

    let simple_blob_detector = SimpleBlobDetector::create(params)?;
    let feature2d_detector: Ptr<Feature2D> = Ptr::from(simple_blob_detector);
    let mut centers = Vector::<Point2f>::default();

    let pattern_found = find_circles_grid(
        &im,
        board_size,
        &mut centers,
        if asymmetric {
            CALIB_CB_ASYMMETRIC_GRID
        } else {
            CALIB_CB_SYMMETRIC_GRID
        } | CALIB_CB_CLUSTERING,
        Some(&feature2d_detector),
        circle_grid_finder_params,
    )?;

    if show {
        display_found_circles(&im, board_size, &mut centers, pattern_found, port)?;
    }

    let size = im.input_array()?.size(-1)?;
    for center in centers.as_mut_slice() {
        center.x *= (object_resolution.0 - 1) as f32 / (size.width - 1) as f32;
        center.y *= (object_resolution.1 - 1) as f32 / (size.height - 1) as f32;
    }
    Ok(PatternPoints {
        points: centers,
        pattern_found,
    })
}

fn display_found_circles(
    im: &impl ToInputArray,
    board_size: opencv::core::Size,
    centers: &Vector<Point2f>,
    pattern_was_found: bool,
    port: Port,
) -> Result<(), ViewerError> {
    let mut centers = centers.clone();
    let mut display_im = Mat::default();
    cvt_color_def(im, &mut display_im, COLOR_GRAY2BGR)?;
    let tmp = display_im;
    let mut display_im = Mat::default();
    resize(
        &tmp,
        &mut display_im,
        Size::new(512, 512),
        0.0,
        0.0,
        INTER_CUBIC,
    )?;
    centers.as_mut_slice().iter_mut().for_each(|x| {
        *x *= 512.0 / 98. as f32;
        let half_pixel = (512.0 / 98. as f32) / 2.;
        *x += (half_pixel, half_pixel).into();
    });

    draw_chessboard_corners(&mut display_im, board_size, &centers, pattern_was_found)?;

    let name = match port {
        Port::Wf => "wf",
        Port::Nf => "nf",
    };
    imshow(name, &display_im)?;
    poll_key()?;
    Ok(())
}

pub(crate) fn board_points(
    board_rows: u16,
    board_cols: u16,
    spacing: f32,
    asymmetric: bool,
) -> Vector<opencv::core::Point3_<f32>> {
    let mut board_points = Vector::<Point3f>::new();

    let step = if asymmetric {
        spacing / 2.0_f32.sqrt()
    } else {
        spacing
    };
    for row in 0..board_rows {
        for col in 0..board_cols {
            let x = if asymmetric {
                (2 * col + row % 2) as f32 * step
            } else {
                col as f32 * step
            };
            board_points.push(Point3f::new(x, row as f32 * step, 0.0));
        }
    }

    board_points
}
