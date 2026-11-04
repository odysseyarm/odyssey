use nalgebra::{Point2, RealField, Scalar};
use num::FromPrimitive;
use num::traits::Float;

pub const MAX_SCREEN_ID: u8 = 5;
pub const MARKER_PATTERN_LEN: usize = 6;

/// `rotation` describes the rotation to transform coordinates in world space to physical screen
/// space.
///
/// `object_points` is the positions of the markers in meters in physical screen space, where the
/// physical screen lies on the XY plane.
///
/// `homography` is used to map from physical screen space to
/// the unit square, i.e. go from `(x, y)` in meters to `(x', y')` where `x', y' ∈ [0, 1]`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy)]
pub struct ScreenCalibration<F: Float + FromPrimitive + Scalar + RealField> {
    #[cfg_attr(feature = "serde", serde(default))]
    pub rotation: nalgebra::UnitQuaternion<F>,
    pub homography: nalgebra::Matrix3<F>,
    pub object_points: [nalgebra::Point3<F>; MARKER_PATTERN_LEN],
}

impl<F: Float + FromPrimitive + Scalar + RealField> ScreenCalibration<F> {
    pub fn cast<G>(&self) -> ScreenCalibration<G>
    where
        G: Float + FromPrimitive + Scalar + RealField,
        F: simba::scalar::SubsetOf<G>,
    {
        ScreenCalibration {
            rotation: self.rotation.cast(),
            homography: self.homography.cast(),
            object_points: self.object_points.map(nalgebra::OPoint::cast),
        }
    }

    /// tl, tr, bl, br
    pub fn bounds(&self) -> [Point2<F>; 4] {
        let _0 = F::from_f32(0.).unwrap();
        let _1 = F::from_f32(1.).unwrap();
        let inv_homography = self.homography.try_inverse().unwrap();
        [
            Point2::new(_0, _0),
            Point2::new(_1, _0),
            Point2::new(_0, _1),
            Point2::new(_1, _1),
        ]
        .map(|p| inv_homography.transform_point(&p))
    }
}
