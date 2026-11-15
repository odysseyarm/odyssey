#![expect(
    unsafe_code,
    reason = "OpenCV matrix conversion initializes raw matrix storage"
)]
use nalgebra::{Dim, Matrix, Scalar, Storage};
use opencv::core::{CV_32FC1, Mat, MatTrait};
use simba::scalar::SubsetOf;

/// Convert a matrix to CV_32FC1.
///
/// # Errors
/// Returns an OpenCV error if dimensions exceed OpenCV limits or allocation fails.
///
/// Since: Odyssey Desktop 0.6.0.
pub fn nalg_to_mat<T, R, C, S>(mat: &Matrix<T, R, C, S>) -> opencv::Result<Mat>
where
    T: SubsetOf<f32> + Scalar,
    R: Dim,
    C: Dim,
    S: Storage<T, R, C>,
{
    let rows = i32::try_from(mat.nrows()).map_err(|_| {
        opencv::Error::new(
            opencv::core::StsOutOfRange,
            "row count exceeds OpenCV limits",
        )
    })?;
    let cols = i32::try_from(mat.ncols()).map_err(|_| {
        opencv::Error::new(
            opencv::core::StsOutOfRange,
            "column count exceeds OpenCV limits",
        )
    })?;
    // SAFETY: dimensions are nonnegative and allocation is initialized fully below before any read.
    let mut r = unsafe { Mat::new_rows_cols(rows, cols, CV_32FC1)? };
    let ptr = r.data_mut() as *mut f32;
    for (i, v) in mat.row_iter().flatten().enumerate() {
        // SAFETY: i indexes an element in the newly allocated contiguous matrix.
        let slot = unsafe { ptr.add(i) };
        // SAFETY: every matrix slot receives exactly one f32 value before the Mat is read.
        unsafe { slot.write(nalgebra::convert_ref(v)) };
    }
    Ok(r)
}
