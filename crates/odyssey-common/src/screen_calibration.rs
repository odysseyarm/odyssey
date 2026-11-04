use nalgebra::{Point2, RealField, Scalar};
use num::FromPrimitive;
use num::traits::Float;

#[cfg(feature = "std")]
use crate::screen_surface::{ScreenSurface, ScreenSurfaceError, SurfaceHit, plane_hit};

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
/// `(x', y')` with `x', y' ∈ [0, 1]`. It is the planar fit of the display.
///
/// `surface`, when present, is the measured display: it replaces the plane and
/// homography for aim mapping wherever the aim ray hits it.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(not(feature = "std"), derive(Copy))]
#[derive(Debug, Clone)]
pub struct ScreenCalibration<F: Float + FromPrimitive + Scalar + RealField> {
    #[cfg_attr(feature = "serde", serde(default))]
    pub rotation: nalgebra::UnitQuaternion<F>,
    pub homography: nalgebra::Matrix3<F>,
    pub object_points: [nalgebra::Point3<F>; MARKER_PATTERN_LEN],
    #[cfg(feature = "std")]
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub surface: Option<ScreenSurface>,
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
            #[cfg(feature = "std")]
            surface: self.surface.clone(),
        }
    }

    /// Aim on the display for a ray in physical-screen space.
    ///
    /// With a surface: the nearest hit on it, inside `[0, 1]^2`. Without one, or
    /// when the ray misses it: the `z = 0` plane through `homography`, which may
    /// fall outside the unit square (off-screen aim). The distance is along the
    /// ray in units of `direction`'s length, matching the planar path.
    #[cfg(feature = "std")]
    pub fn aim_from_ray(
        &self,
        origin: &nalgebra::Vector3<F>,
        direction: &nalgebra::Vector3<F>,
    ) -> Option<(Point2<F>, F)> {
        if let Some(surface) = &self.surface
            && let Some(hit) = surface.raycast(origin.map(as_f64), direction.map(as_f64))
        {
            let length = as_f64(direction.norm());
            return Some((
                hit.uv.map(|v| F::from_f64(v).unwrap_or_else(F::nan)),
                F::from_f64(hit.distance_m / length)?,
            ));
        }
        let t = -origin.z / direction.z;
        let intersection = origin + direction.scale(t);
        Some((
            self.homography
                .transform_point(&Point2::new(intersection.x, intersection.y)),
            t,
        ))
    }

    /// Physical-screen point shown at `uv`, on the surface when present.
    #[cfg(feature = "std")]
    pub fn point_at_uv(&self, uv: Point2<F>) -> Option<nalgebra::Point3<F>> {
        if let Some(surface) = &self.surface {
            let uv = uv.map(as_f64);
            return surface
                .point_at_uv(uv)
                .map(|p| nalgebra::Point3::from(p.map(|v| F::from_f64(v).unwrap_or_else(F::nan))));
        }
        let p = self.homography.try_inverse()?.transform_point(&uv);
        Some(nalgebra::Point3::new(p.x, p.y, F::zero()))
    }

    /// Nearest display hit inside `[0, 1]^2`, with distance in meters. Unlike
    /// [`Self::aim_from_ray`], a miss is `None`; used for reference labels.
    #[cfg(feature = "std")]
    pub fn surface_hit(
        &self,
        origin: nalgebra::Vector3<f64>,
        direction: nalgebra::Vector3<f64>,
    ) -> Option<SurfaceHit> {
        match &self.surface {
            Some(surface) => surface.raycast(origin, direction),
            None => plane_hit(&self.homography.map(as_f64), origin, direction),
        }
    }
}

#[cfg(feature = "std")]
fn as_f64<F: num::ToPrimitive>(value: F) -> f64 {
    value.to_f64().unwrap_or(f64::NAN)
}

impl<F: Float + FromPrimitive + Scalar + RealField> ScreenCalibration<F> {
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
    #[cfg(feature = "std")]
    Surface(ScreenSurfaceError),
}

impl core::fmt::Display for ScreenCalibrationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NonFiniteCoordinates => "screen calibration contains non-finite coordinates",
            Self::InvalidBounds => "screen calibration has singular or infinite bounds",
            Self::InvalidRotation => "screen calibration rotation is not a finite unit quaternion",
            #[cfg(feature = "std")]
            Self::Surface(error) => return write!(f, "screen calibration surface: {error}"),
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
        #[cfg(feature = "std")]
        if let Some(surface) = &self.surface {
            surface
                .validate()
                .map_err(ScreenCalibrationError::Surface)?;
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
            surface: None,
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
            surface: None,
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
