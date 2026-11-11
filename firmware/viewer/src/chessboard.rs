use opencv::calib3d::{find_chessboard_corners, find_chessboard_corners_sb};
use opencv::core::{
    _InputArrayTraitConst, FileStorage, FileStorage_READ, Mat, Point2f, Size, TermCriteria_EPS,
    TermCriteria_MAX_ITER, ToInputArray, Vector,
};
use opencv::hub_prelude::{FileNodeTraitConst, FileStorageTraitConst};
use opencv::imgproc::{INTER_CUBIC, corner_sub_pix, resize};

use crate::{Port, ViewerError};

/// Read intrinsics, preserving OpenCV failures and rejecting unopened files.
///
/// Since: Odyssey Desktop 0.6.0.
pub fn read_camera_params(path: &str) -> Result<(Mat, Mat), ViewerError> {
    let fs = FileStorage::new_def(path, FileStorage_READ)?;
    if !fs.is_opened()? {
        return Err(ViewerError::CalibrationFile { path: path.into() });
    }
    let camera_matrix = fs.get("camera_matrix")?.mat()?;
    let dist_coeffs = fs.get("dist_coeffs")?.mat()?;
    Ok((camera_matrix, dist_coeffs))
}

/// Returns `Ok(None)` when no corners are found, or the OpenCV failure.
///
/// Since: Odyssey Desktop 0.6.0.
pub fn get_chessboard_corners(
    image: &[u8],
    port: Port,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    board_rows: u16,
    board_cols: u16,
    show: bool,
) -> Result<Option<Vector<Point2f>>, ViewerError> {
    let im = Mat::new_rows_cols_with_data(resolution.1.into(), resolution.0.into(), image)?;
    get_chessboard_corners_cv(&im, port, object_resolution, board_rows, board_cols, show)
}

/// Since: Odyssey Desktop 0.6.0.
pub fn get_chessboard_corners_cv(
    image: &impl ToInputArray,
    _port: Port,
    object_resolution: (u32, u32),
    board_rows: u16,
    board_cols: u16,
    _show: bool,
) -> Result<Option<Vector<Point2f>>, ViewerError> {
    let board_rows = i32::from(board_rows);
    let board_cols = i32::from(board_cols);

    let mut corners = Vector::<Point2f>::default();
    let use_sb = true;
    let mut im_upscaled = Mat::default();
    // super resolution scale
    let ss;
    if image.input_array()?.size_def()? == (98, 98).into() {
        ss = 4.;
        resize(
            image,
            &mut im_upscaled,
            Size::new(0, 0),
            ss,
            ss,
            INTER_CUBIC,
        )?;
    } else {
        ss = 1.;
        im_upscaled = image.input_array()?.get_mat_def()?;
    }
    if use_sb {
        let _chessboard_found = find_chessboard_corners_sb(
            &im_upscaled,
            (board_cols, board_rows).into(),
            &mut corners,
            0,
        )?;
    } else {
        let _chessboard_found = find_chessboard_corners(
            &im_upscaled,
            (board_cols, board_rows).into(),
            &mut corners,
            0,
        )?;
        if corners.len() > 0 {
            corner_sub_pix(
                &im_upscaled,
                &mut corners,
                (3, 3).into(),
                (-1, -1).into(),
                opencv::core::TermCriteria {
                    typ: TermCriteria_EPS + TermCriteria_MAX_ITER,
                    max_count: 30,
                    epsilon: 0.001,
                },
            )?;
        }
    }
    // Back to input-image pixels. The ±0.5 shifts between pixel-centre and pixel-corner origins.
    corners.as_mut_slice().iter_mut().for_each(|x| {
        x.x = (x.x + 0.5) / ss as f32 - 0.5;
        x.y = (x.y + 0.5) / ss as f32 - 0.5;
    });

    if corners.len() > 0 {
        let size = image.input_array()?.size(-1)?;
        for corner in corners.as_mut_slice() {
            corner.x *= (object_resolution.0 - 1) as f32 / (size.width - 1) as f32;
            corner.y *= (object_resolution.1 - 1) as f32 / (size.height - 1) as f32;
        }
        Ok(Some(corners))
    } else {
        Ok(None)
    }
}
