use nalgebra::{Isometry3, Matrix3, RealField, Rotation3, Translation3, Vector3, Vector5};
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy)]
pub struct OpenCVMatrix3<R: RealField> {
    pub data: [R; 9],
}

impl<R: RealField> From<Matrix3<R>> for OpenCVMatrix3<R> {
    fn from(mat: Matrix3<R>) -> Self {
        OpenCVMatrix3 {
            data: [
                mat[(0, 0)].clone(),
                mat[(0, 1)].clone(),
                mat[(0, 2)].clone(),
                mat[(1, 0)].clone(),
                mat[(1, 1)].clone(),
                mat[(1, 2)].clone(),
                mat[(2, 0)].clone(),
                mat[(2, 1)].clone(),
                mat[(2, 2)].clone(),
            ],
        }
    }
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct OpenCVMatrix3WithRowsAndCols<R: RealField> {
    #[cfg(feature = "std")]
    #[serde(default)]
    pub rows: usize,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub cols: usize,

    #[cfg(not(feature = "std"))]
    pub rows: usize,
    #[cfg(not(feature = "std"))]
    pub cols: usize,

    pub data: [R; 9],
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy)]
pub struct OpenCVMatrix3x1<R: RealField> {
    pub data: [R; 3],
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct OpenCVMatrix3x1WithRowsAndCols<R: RealField> {
    #[cfg(feature = "std")]
    #[serde(default)]
    pub rows: usize,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub cols: usize,

    #[cfg(not(feature = "std"))]
    pub rows: usize,
    #[cfg(not(feature = "std"))]
    pub cols: usize,

    pub data: [R; 3],
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone, Copy)]
pub struct OpenCVMatrix5x1<R: RealField> {
    pub data: [R; 5],
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct OpenCVMatrix5x1WithRowsAndCols<R: RealField> {
    #[cfg(feature = "std")]
    #[serde(default)]
    pub rows: usize,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub cols: usize,

    #[cfg(not(feature = "std"))]
    pub rows: usize,
    #[cfg(not(feature = "std"))]
    pub cols: usize,

    pub data: [R; 5],
}

impl<R: RealField> From<[R; 9]> for OpenCVMatrix3<R> {
    fn from(data: [R; 9]) -> Self {
        OpenCVMatrix3 { data }
    }
}

impl<R: RealField> From<[R; 3]> for OpenCVMatrix3x1<R> {
    fn from(data: [R; 3]) -> Self {
        OpenCVMatrix3x1 { data }
    }
}

impl<R: RealField> From<[R; 5]> for OpenCVMatrix5x1<R> {
    fn from(data: [R; 5]) -> Self {
        OpenCVMatrix5x1 { data }
    }
}

impl<R: RealField> From<Vector5<R>> for OpenCVMatrix5x1<R> {
    fn from(data: Vector5<R>) -> Self {
        OpenCVMatrix5x1 { data: data.into() }
    }
}

impl<R: RealField> From<Vector3<R>> for OpenCVMatrix3x1<R> {
    fn from(data: Vector3<R>) -> Self {
        OpenCVMatrix3x1 { data: data.into() }
    }
}

impl<R: RealField> From<OpenCVMatrix3<R>> for [R; 9] {
    fn from(mat: OpenCVMatrix3<R>) -> Self {
        mat.data
    }
}

impl<R: RealField> From<OpenCVMatrix3x1<R>> for [R; 3] {
    fn from(mat: OpenCVMatrix3x1<R>) -> Self {
        mat.data
    }
}

impl<R: RealField> From<OpenCVMatrix5x1<R>> for [R; 5] {
    fn from(mat: OpenCVMatrix5x1<R>) -> Self {
        mat.data
    }
}

impl<R: RealField> From<OpenCVMatrix3WithRowsAndCols<R>> for OpenCVMatrix3<R> {
    fn from(mat: OpenCVMatrix3WithRowsAndCols<R>) -> Self {
        OpenCVMatrix3 { data: mat.data }
    }
}

impl<R: RealField> From<OpenCVMatrix3x1WithRowsAndCols<R>> for OpenCVMatrix3x1<R> {
    fn from(mat: OpenCVMatrix3x1WithRowsAndCols<R>) -> Self {
        OpenCVMatrix3x1 { data: mat.data }
    }
}

impl<R: RealField> From<OpenCVMatrix5x1WithRowsAndCols<R>> for OpenCVMatrix5x1<R> {
    fn from(mat: OpenCVMatrix5x1WithRowsAndCols<R>) -> Self {
        OpenCVMatrix5x1 { data: mat.data }
    }
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug)]
pub struct CameraCalibrationParams<R: RealField> {
    pub camera_matrix: OpenCVMatrix3WithRowsAndCols<R>,
    pub dist_coeffs: OpenCVMatrix5x1WithRowsAndCols<R>,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub rms_error: f64,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub num_captures: usize,

    #[cfg(not(feature = "std"))]
    pub rms_error: f64,
    #[cfg(not(feature = "std"))]
    pub num_captures: usize,
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Debug)]
pub struct StereoCalibrationParams<R: RealField> {
    pub r: OpenCVMatrix3WithRowsAndCols<R>,
    pub t: OpenCVMatrix3x1WithRowsAndCols<R>,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub rms_error: f64,
    #[cfg(feature = "std")]
    #[serde(default)]
    pub num_captures: usize,

    #[cfg(not(feature = "std"))]
    pub rms_error: f64,
    #[cfg(not(feature = "std"))]
    pub num_captures: usize,
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Clone, Copy)]
pub struct MinimalCameraCalibrationParams<R: RealField> {
    pub camera_matrix: OpenCVMatrix3<R>,
    pub dist_coeffs: OpenCVMatrix5x1<R>,
}

#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct MinimalStereoCalibrationParams<R: RealField> {
    pub r: OpenCVMatrix3<R>,
    pub t: OpenCVMatrix3x1<R>,
}

impl MinimalCameraCalibrationParams<f32> {
    #[cfg(feature = "std")]
    pub fn parse(
        data: &mut &[u8],
    ) -> Result<MinimalCameraCalibrationParams<f32>, Box<dyn std::error::Error>> {
        let mut params = MinimalCameraCalibrationParams {
            camera_matrix: OpenCVMatrix3 { data: [0.; 9] },
            dist_coeffs: OpenCVMatrix5x1 { data: [0.; 5] },
        };
        let mut data_iter = data.iter();
        for i in 0..9 {
            params.camera_matrix.data[i] = f32::from_le_bytes([
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
            ]);
        }
        for i in 0..5 {
            params.dist_coeffs.data[i] = f32::from_le_bytes([
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
            ]);
        }
        *data = data_iter.as_slice();
        Ok(params)
    }

    #[cfg(feature = "std")]
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        for i in 0..9 {
            buf.extend_from_slice(&self.camera_matrix.data[i].to_le_bytes());
        }
        for i in 0..5 {
            buf.extend_from_slice(&self.dist_coeffs.data[i].to_le_bytes());
        }
    }
}

impl MinimalStereoCalibrationParams<f32> {
    #[cfg(feature = "std")]
    pub fn parse(
        data: &mut &[u8],
    ) -> Result<MinimalStereoCalibrationParams<f32>, Box<dyn std::error::Error>> {
        let mut params = MinimalStereoCalibrationParams {
            r: OpenCVMatrix3 { data: [0.; 9] },
            t: OpenCVMatrix3x1 { data: [0.; 3] },
        };
        let mut data_iter = data.iter();
        for i in 0..9 {
            params.r.data[i] = f32::from_le_bytes([
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
            ]);
        }
        for i in 0..3 {
            params.t.data[i] = f32::from_le_bytes([
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
                *data_iter.next().ok_or("Unexpected end of data")?,
            ]);
        }
        *data = &data_iter.as_slice();
        Ok(params)
    }

    #[cfg(feature = "std")]
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        for i in 0..9 {
            buf.extend_from_slice(&self.r.data[i].to_le_bytes());
        }
        for i in 0..3 {
            buf.extend_from_slice(&self.t.data[i].to_le_bytes());
        }
    }
}

impl<R: RealField> From<MinimalCameraCalibrationParams<R>> for CameraCalibrationParams<R> {
    fn from(minimal: MinimalCameraCalibrationParams<R>) -> Self {
        CameraCalibrationParams {
            camera_matrix: OpenCVMatrix3WithRowsAndCols {
                rows: 3,
                cols: 3,
                data: minimal.camera_matrix.data,
            },
            dist_coeffs: OpenCVMatrix5x1WithRowsAndCols {
                rows: 5,
                cols: 1,
                data: minimal.dist_coeffs.data,
            },
            rms_error: 0.,
            num_captures: 0,
        }
    }
}

#[cfg(feature = "std")]
impl<R: RealField> From<MinimalStereoCalibrationParams<R>> for StereoCalibrationParams<R> {
    fn from(minimal: MinimalStereoCalibrationParams<R>) -> Self {
        StereoCalibrationParams {
            r: OpenCVMatrix3WithRowsAndCols {
                rows: 3,
                cols: 3,
                data: minimal.r.data,
            },
            t: OpenCVMatrix3x1WithRowsAndCols {
                rows: 3,
                cols: 1,
                data: minimal.t.data,
            },
            rms_error: 0.,
            num_captures: 0,
        }
    }
}

impl<R: RealField> From<MinimalCameraCalibrationParams<R>> for RosOpenCvIntrinsics<R> {
    fn from(val: MinimalCameraCalibrationParams<R>) -> Self {
        let camera_calibration_params: CameraCalibrationParams<R> = val.into();
        camera_calibration_params.into()
    }
}

impl<R: RealField> From<MinimalStereoCalibrationParams<R>> for Isometry3<R> {
    fn from(val: MinimalStereoCalibrationParams<R>) -> Self {
        let r = Matrix3::from_row_slice(&val.r.data);
        let t = Vector3::from_column_slice(&val.t.data);

        Isometry3::from_parts(
            Translation3::from(t),
            Rotation3::from_matrix_unchecked(r).into(),
        )
    }
}

impl<R: RealField> From<StereoCalibrationParams<R>> for MinimalStereoCalibrationParams<R> {
    fn from(params: StereoCalibrationParams<R>) -> Self {
        MinimalStereoCalibrationParams {
            r: OpenCVMatrix3 {
                data: params.r.data,
            },
            t: OpenCVMatrix3x1 {
                data: params.t.data,
            },
        }
    }
}

impl<R: RealField> From<CameraCalibrationParams<R>> for RosOpenCvIntrinsics<R> {
    fn from(val: CameraCalibrationParams<R>) -> Self {
        let distortion = Distortion::from_opencv_vec(Vector5::from(val.dist_coeffs.data.clone()));

        let fx = val.camera_matrix.data[0].clone();
        let skew = val.camera_matrix.data[1].clone();
        let fy = val.camera_matrix.data[4].clone();
        let cx = val.camera_matrix.data[2].clone();
        let cy = val.camera_matrix.data[5].clone();

        RosOpenCvIntrinsics::from_params_with_distortion(fx, skew, fy, cx, cy, distortion)
    }
}

impl<R: RealField> From<RosOpenCvIntrinsics<R>> for MinimalCameraCalibrationParams<R> {
    fn from(intrinsics: RosOpenCvIntrinsics<R>) -> Self {
        let camera_matrix = intrinsics.p;
        let dist_coeffs = OpenCVMatrix5x1 {
            data: intrinsics.distortion.opencv_vec().clone().cast().into(),
        };

        MinimalCameraCalibrationParams {
            camera_matrix: OpenCVMatrix3 {
                data: [
                    camera_matrix[(0, 0)].clone(),
                    camera_matrix[(0, 1)].clone(),
                    camera_matrix[(0, 2)].clone(),
                    camera_matrix[(1, 0)].clone(),
                    camera_matrix[(1, 1)].clone(),
                    camera_matrix[(1, 2)].clone(),
                    camera_matrix[(2, 0)].clone(),
                    camera_matrix[(2, 1)].clone(),
                    camera_matrix[(2, 2)].clone(),
                ],
            },
            dist_coeffs,
        }
    }
}

impl<R: RealField> From<StereoCalibrationParams<R>> for Isometry3<R> {
    fn from(val: StereoCalibrationParams<R>) -> Self {
        let r = Matrix3::from_row_slice(&val.r.data);
        let t = Vector3::from_column_slice(&val.t.data);

        Isometry3::from_parts(
            Translation3::from(t),
            Rotation3::from_matrix_unchecked(r).into(),
        )
    }
}

#[cfg(feature = "std")]
impl<R: RealField> From<Isometry3<R>> for MinimalStereoCalibrationParams<R> {
    fn from(isometry: Isometry3<R>) -> Self {
        let binding = isometry.rotation.to_rotation_matrix();
        let r = binding.matrix().to_owned();
        let t = isometry.translation.vector;

        MinimalStereoCalibrationParams {
            r: r.into(),
            t: OpenCVMatrix3x1 { data: t.into() },
        }
    }
}
