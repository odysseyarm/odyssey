use nalgebra::{Point2, RealField, Scalar};
use num::FromPrimitive;
use num::traits::Float;

/// Highest screen id a hub tracks; ids run `0..=MAX_SCREEN_ID`.
pub const MAX_SCREEN_ID: u8 = 5;
/// Number of markers in the pattern shown on a screen.
pub const MARKER_PATTERN_LEN: usize = 6;

/// `rotation` transforms coordinates from world space to physical screen space.
///
/// `object_points` are the marker positions in meters in physical screen space, where the
/// screen lies on the XY plane.
///
/// `homography` maps physical screen space to the unit square: `(x, y)` in meters to
/// `(x', y')` with `x', y' ∈ [0, 1]`.
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

    /// Top-left, top-right, bottom-left and bottom-right corners.
    ///
    /// Returns `None` for a singular homography, non-finite corners, or a
    /// projective horizon crossing the unit square.
    pub fn bounds(&self) -> Option<[Point2<F>; 4]> {
        let zero = <F as num::Zero>::zero();
        let one = <F as num::One>::one();
        let inv_homography = self.homography.try_inverse()?;
        let mut corners = [
            Point2::new(zero, zero),
            Point2::new(one, zero),
            Point2::new(zero, one),
            Point2::new(one, one),
        ];
        let mut positive_side = None;
        for corner in &mut corners {
            let homogeneous = inv_homography * nalgebra::Vector3::new(corner.x, corner.y, one);
            let positive = homogeneous.z > zero;
            if homogeneous.z == zero
                || !homogeneous.iter().all(|&v| Float::is_finite(v))
                || positive_side.is_some_and(|side| side != positive)
            {
                return None;
            }
            positive_side = Some(positive);
            *corner = Point2::new(homogeneous.x / homogeneous.z, homogeneous.y / homogeneous.z);
            if !Float::is_finite(corner.x) || !Float::is_finite(corner.y) {
                return None;
            }
        }
        Some(corners)
    }
}

/// Invalid persisted screen calibration data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCalibrationError {
    NonFiniteCoordinates,
    InvalidBounds,
    InvalidRotation,
}

impl core::fmt::Display for ScreenCalibrationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NonFiniteCoordinates => "screen calibration contains non-finite coordinates",
            Self::InvalidBounds => "screen calibration has singular or infinite bounds",
            Self::InvalidRotation => "screen calibration rotation is not a finite unit quaternion",
        })
    }
}

impl core::error::Error for ScreenCalibrationError {}

impl ScreenCalibration<f32> {
    /// Checks data loaded from an untrusted calibration file before use by the
    /// tracker. Deserialization alone does not enforce these numeric invariants.
    pub fn validate(&self) -> Result<(), ScreenCalibrationError> {
        if !self.homography.iter().all(|v| v.is_finite())
            || !self
                .object_points
                .iter()
                .all(|p| p.iter().all(|v| v.is_finite()))
        {
            return Err(ScreenCalibrationError::NonFiniteCoordinates);
        }
        if self.bounds().is_none() {
            return Err(ScreenCalibrationError::InvalidBounds);
        }
        let rotation = self.rotation.quaternion();
        // Allow f32 serialization/normalization roundoff, not arbitrary scales.
        if !rotation.coords.iter().all(|v| v.is_finite())
            || (rotation.norm_squared() - 1.0).abs() > 1e-4
        {
            return Err(ScreenCalibrationError::InvalidRotation);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use nalgebra::{Matrix3, Point3, UnitQuaternion};

    use super::*;

    #[test]
    fn bounds_reject_singular_and_infinite_corners() {
        let mut calibration = ScreenCalibration {
            rotation: UnitQuaternion::<f32>::identity(),
            homography: Matrix3::zeros(),
            object_points: [Point3::origin(); MARKER_PATTERN_LEN],
        };
        assert!(calibration.bounds().is_none());
        // Inverse maps the right-hand corners to z = 0.
        calibration.homography = Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0);
        assert!(calibration.bounds().is_none());
        // All four corners are finite but the horizon crosses the interior.
        calibration.homography = Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 1.0);
        assert!(calibration.bounds().is_none());
        calibration.homography = Matrix3::new(0.5, 0.0, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 1.0);
        assert_eq!(
            calibration.bounds().unwrap(),
            [
                Point2::new(0.0, 0.0),
                Point2::new(2.0, 0.0),
                Point2::new(0.0, 4.0),
                Point2::new(2.0, 4.0),
            ]
        );
    }

    #[test]
    fn validation_rejects_bad_markers_and_unnormalized_rotations() {
        let mut calibration = ScreenCalibration {
            rotation: UnitQuaternion::<f32>::identity(),
            homography: Matrix3::identity(),
            object_points: [Point3::origin(); MARKER_PATTERN_LEN],
        };
        assert_eq!(calibration.validate(), Ok(()));
        calibration.object_points[0].x = f32::NAN;
        assert_eq!(
            calibration.validate(),
            Err(ScreenCalibrationError::NonFiniteCoordinates)
        );
        calibration.object_points[0].x = 0.0;
        calibration.rotation =
            UnitQuaternion::new_unchecked(nalgebra::Quaternion::new(2.0, 0.0, 0.0, 0.0));
        assert_eq!(
            calibration.validate(),
            Err(ScreenCalibrationError::InvalidRotation)
        );
    }
}
