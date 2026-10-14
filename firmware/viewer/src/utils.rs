use nalgebra::{Dim, Matrix, Scalar, Storage};
use opencv::core::{Mat, MatTrait, CV_32FC1};
use simba::scalar::SubsetOf;

/// Convert a [`nalgebra::Matrix`] to a CV_32FC1 [`opencv::core::Mat`]
pub fn nalg_to_mat<T, R, C, S>(mat: &Matrix<T, R, C, S>) -> Mat
where
    T: SubsetOf<f32> + Scalar,
    R: Dim,
    C: Dim,
    S: Storage<T, R, C>,
{
    unsafe {
        let mut r = Mat::new_rows_cols(mat.nrows() as i32, mat.ncols() as i32, CV_32FC1).unwrap();
        let ptr = r.data_mut() as *mut f32;
        for (i, v) in mat.row_iter().flatten().enumerate() {
            *ptr.add(i) = nalgebra::convert_ref(v);
        }
        r
    }
}
