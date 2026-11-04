//! World-space view of a scene screen's calibrated display.
//!
//! The display geometry lives in `ScreenCalibration` (the planar homography and
//! the optional measured surface grid); this module adds the scene's world frame
//! and the marker identity checks a scene needs on top of calibration validity.

use nalgebra::{Matrix3, Point2, UnitQuaternion, Vector3};
use odyssey_common::ScreenCalibrationError;

use crate::dataset::SceneScreen;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SurfaceError {
    #[error("invalid screen calibration: {0}")]
    Calibration(#[from] ScreenCalibrationError),
    #[error("invalid screen ID, marker IDs or canonical marker geometry")]
    Markers,
    #[error("UV must be finite and in [0,1]^2")]
    Uv,
    #[error("ray origin and nonzero direction must be finite")]
    Ray,
}

/// Nearest forward hit. Distance is meters regardless of input direction length.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceHit {
    pub uv: [f64; 2],
    pub position_world_m: [f64; 3],
    pub distance_m: f64,
}

/// Validated borrowed geometry. Construct once with [`SceneScreen::geometry`], then
/// reuse for rays/UV samples; validation scans grids, queries do not allocate.
#[derive(Debug)]
pub struct ScreenGeometry<'a> {
    screen: &'a SceneScreen,
    inverse_homography: Matrix3<f64>,
    world_to_screen: UnitQuaternion<f64>,
}

impl SceneScreen {
    /// Validate untrusted calibration (including its surface) and marker identities.
    /// Markers live only in `calibration.object_points`, in physical-screen space.
    pub fn geometry(&self) -> Result<ScreenGeometry<'_>, SurfaceError> {
        self.calibration.validate()?;
        let points = &self.calibration.object_points;
        if self.id > odyssey_common::MAX_SCREEN_ID
            || self.marker_ids.iter().enumerate().any(|(i, id)| {
                id.trim().is_empty() || id.len() > 4096 || self.marker_ids[..i].contains(id)
            })
            || points.iter().enumerate().any(|(i, p)| {
                points[..i]
                    .iter()
                    .any(|q| (p.cast::<f64>() - q.cast::<f64>()).norm_squared() <= 1e-12)
            })
            || !points[2..].iter().any(|point| {
                (points[1].cast::<f64>() - points[0].cast::<f64>())
                    .cross(&(point.cast::<f64>() - points[0].cast::<f64>()))
                    .norm_squared()
                    > 1e-12
            })
        {
            return Err(SurfaceError::Markers);
        }
        let inverse_homography = self
            .calibration
            .homography
            .cast::<f64>()
            .try_inverse()
            .ok_or(SurfaceError::Calibration(
                ScreenCalibrationError::InvalidBounds,
            ))?;
        Ok(ScreenGeometry {
            screen: self,
            inverse_homography,
            world_to_screen: UnitQuaternion::new_normalize(
                self.calibration.rotation.quaternion().cast::<f64>(),
            ),
        })
    }
}

impl ScreenGeometry<'_> {
    /// Physical-screen meters to world meters. Calibration has rotation, no translation.
    pub fn screen_to_world(&self, point_m: [f64; 3]) -> [f64; 3] {
        self.world_to_screen
            .inverse_transform_vector(&Vector3::from(point_m))
            .into()
    }

    /// World meters to physical-screen meters, using calibration's world->screen rotation.
    pub fn world_to_screen(&self, point_m: [f64; 3]) -> [f64; 3] {
        self.world_to_screen
            .transform_vector(&Vector3::from(point_m))
            .into()
    }

    /// Canonical marker in world meters; index follows `marker_ids` / `object_points`.
    pub fn marker_world(&self, index: usize) -> Option<[f64; 3]> {
        self.screen
            .calibration
            .object_points
            .get(index)
            .map(|p| self.screen_to_world([f64::from(p.x), f64::from(p.y), f64::from(p.z)]))
    }

    /// Inverse display mapping. Plane uses inverse homography (projective, not
    /// bilinear corner interpolation); a surface uses its prescribed triangles.
    pub fn point_at_uv(&self, uv: [f64; 2]) -> Result<[f64; 3], SurfaceError> {
        if !uv.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
            return Err(SurfaceError::Uv);
        }
        let point = match &self.screen.calibration.surface {
            Some(surface) => surface
                .point_at_uv(Point2::from(uv))
                .ok_or(SurfaceError::Uv)?,
            None => {
                let p = self.inverse_homography * Vector3::new(uv[0], uv[1], 1.0);
                Vector3::new(p.x / p.z, p.y / p.z, 0.0)
            }
        };
        Ok(self.screen_to_world(point.into()))
    }

    /// Nearest two-sided forward display hit. Miss, parallel or behind returns
    /// `Ok(None)`; nonfinite inputs / zero direction return `SurfaceError::Ray`.
    /// Hit UV always lies in [0,1]^2, including boundary edges and corners.
    pub fn raycast(
        &self,
        origin_world_m: [f64; 3],
        direction_world: [f64; 3],
    ) -> Result<Option<SurfaceHit>, SurfaceError> {
        if !origin_world_m
            .iter()
            .chain(direction_world.iter())
            .all(|v| v.is_finite())
        {
            return Err(SurfaceError::Ray);
        }
        let scale = direction_world.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        if scale == 0.0 {
            return Err(SurfaceError::Ray);
        }
        let direction = self
            .world_to_screen
            .transform_vector(&(Vector3::from(direction_world) / scale).normalize());
        let origin = Vector3::from(self.world_to_screen(origin_world_m));
        Ok(self
            .screen
            .calibration
            .surface_hit(origin, direction)
            .map(|hit| SurfaceHit {
                uv: hit.uv.into(),
                position_world_m: self
                    .screen_to_world((origin + direction * hit.distance_m).into()),
                distance_m: hit.distance_m,
            }))
    }
}

#[cfg(test)]
mod tests {
    use odyssey_common::{ScreenSurface, ScreenSurfaceError};

    use super::*;

    fn screen(surface: Option<ScreenSurface>) -> SceneScreen {
        let mut calibration = crate::synthetic_screen_calibration();
        calibration.surface = surface;
        SceneScreen {
            id: 0,
            calibration,
            marker_ids: std::array::from_fn(|i| format!("screen-0-marker-{i}")),
        }
    }

    fn grid() -> Option<ScreenSurface> {
        Some(ScreenSurface {
            columns: 3,
            rows: 3,
            positions_m: vec![
                [0.0, 0.0, 0.0],
                [0.3, 0.0, 0.1],
                [1.2, 0.0, 0.0],
                [0.0, 0.2, 0.0],
                [0.4, 0.3, 0.25],
                [1.2, 0.4, 0.0],
                [0.0, 0.675, 0.0],
                [0.6, 0.675, -0.1],
                [1.2, 0.675, 0.0],
            ],
        })
    }

    fn surface_error(error: ScreenSurfaceError) -> SurfaceError {
        SurfaceError::Calibration(ScreenCalibrationError::Surface(error))
    }

    fn close(a: [f64; 3], b: [f64; 3]) {
        assert!(
            (Vector3::from(a) - Vector3::from(b)).norm() < 1e-8,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn plane_mapping_is_projective_not_corner_interpolation() {
        let mut screen = screen(None);
        let inverse = Matrix3::new(2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.0, 1.0);
        screen.calibration.homography = inverse.try_inverse().unwrap().cast();
        let geometry = screen.geometry().unwrap();
        for uv in [
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
            [0.5, 0.5],
            [0.2, 0.8],
        ] {
            let denominator = 1.0 + 0.5 * uv[0];
            let expected = [2.0 * uv[0] / denominator, uv[1] / denominator, 0.0];
            close(geometry.point_at_uv(uv).unwrap(), expected);
            let hit = geometry
                .raycast([expected[0], expected[1], -2.0], [0.0, 0.0, 7.0])
                .unwrap()
                .unwrap();
            assert!((hit.uv[0] - uv[0]).abs() < 1e-9);
            assert!((hit.uv[1] - uv[1]).abs() < 1e-9);
            assert!((hit.distance_m - 2.0).abs() < 1e-9);
        }
        let middle = geometry.point_at_uv([0.5, 0.5]).unwrap();
        assert!((middle[0] - 0.8).abs() < 1e-9);
        assert!((middle[0] - 2.0 / 3.0).abs() > 0.1);
    }

    #[test]
    fn wavy_nonlinear_grid_roundtrips_both_triangles_edges_and_vertices() {
        let screen = screen(grid());
        let geometry = screen.geometry().unwrap();
        for uv in [
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
            [0.5, 0.5],
            [0.5, 0.0],
            [1.0, 0.7],
            [0.0, 0.2],
            [0.3, 0.1],
            [0.1, 0.3],
            [0.8, 0.6],
            [0.6, 0.8],
            [0.7, 0.7],
        ] {
            let point = geometry.point_at_uv(uv).unwrap();
            let hit = geometry
                .raycast([point[0], point[1], -2.0], [0.0, 0.0, 1.0])
                .unwrap()
                .unwrap();
            assert!((hit.uv[0] - uv[0]).abs() < 1e-9, "{uv:?} -> {:?}", hit.uv);
            assert!((hit.uv[1] - uv[1]).abs() < 1e-9, "{uv:?} -> {:?}", hit.uv);
            close(hit.position_world_m, point);
            assert!((hit.distance_m - (2.0 + point[2])).abs() < 1e-9);
        }
        close(geometry.point_at_uv([0.5, 0.5]).unwrap(), [0.4, 0.3, 0.25]);
        close(
            geometry.point_at_uv([0.25, 0.125]).unwrap(),
            [0.175, 0.075, 0.0875],
        );
    }

    #[test]
    fn invalid_rays_and_uv_are_errors_but_geometric_misses_are_not() {
        for surface in [None, grid()] {
            let screen = screen(surface);
            let geometry = screen.geometry().unwrap();
            for (origin, direction) in [
                ([0.4, 0.3, -2.0], [0.0; 3]),
                ([f64::NAN, 0.0, 0.0], [0.0, 0.0, 1.0]),
                ([0.0; 3], [f64::INFINITY, 0.0, 1.0]),
            ] {
                assert!(matches!(
                    geometry.raycast(origin, direction),
                    Err(SurfaceError::Ray)
                ));
            }
            for (origin, direction) in [
                ([0.4, 0.3, -2.0], [0.0, 0.0, -1.0]),
                ([-2.0, 0.3, -2.0], [0.0, 0.0, 1.0]),
                ([0.4, 0.3, -2.0], [1.0, 0.0, 0.0]),
            ] {
                assert!(geometry.raycast(origin, direction).unwrap().is_none());
            }
            for uv in [[-0.01, 0.5], [1.01, 0.5], [0.5, f64::NAN]] {
                assert_eq!(geometry.point_at_uv(uv), Err(SurfaceError::Uv));
            }
            let point = geometry.point_at_uv([0.0, 0.0]).unwrap();
            let hit = geometry.raycast(point, [0.0, 0.0, 1.0]).unwrap().unwrap();
            assert_eq!(hit.distance_m, 0.0);
        }
    }

    #[test]
    fn calibration_rotation_applies_to_surface_rays_and_canonical_markers() {
        let mut screen = screen(grid());
        screen.calibration.rotation = UnitQuaternion::from_euler_angles(0.2, -0.4, 0.3);
        screen.calibration.object_points = [
            [0.1, 0.0, 0.1],
            [0.2, 0.0, 0.1],
            [0.3, 0.0, 0.1],
            [0.0, 1.0, 0.3],
            [1.0, 1.0, -0.1],
            [0.5, 1.0, 0.2],
        ]
        .map(nalgebra::Point3::from);
        let geometry = screen.geometry().unwrap();
        let local = [0.4, 0.3, 0.25];
        let world = geometry.point_at_uv([0.5, 0.5]).unwrap();
        close(geometry.world_to_screen(world), local);
        let origin = geometry.screen_to_world([0.4, 0.3, -2.0]);
        let direction = geometry.screen_to_world([0.0, 0.0, 10.0]);
        let hit = geometry.raycast(origin, direction).unwrap().unwrap();
        close(hit.position_world_m, world);
        assert!((hit.uv[0] - 0.5).abs() < 1e-7);
        assert!((hit.uv[1] - 0.5).abs() < 1e-7);
        for (i, point) in screen.calibration.object_points.iter().enumerate() {
            close(
                geometry.world_to_screen(geometry.marker_world(i).unwrap()),
                [f64::from(point.x), f64::from(point.y), f64::from(point.z)],
            );
        }
        assert!(geometry.marker_world(6).is_none());
    }

    #[test]
    fn malformed_grid_sizes_and_nonfinite_positions_are_rejected() {
        for (columns, rows, positions_m) in [
            (1, 2, vec![[0.0; 3]; 2]),
            (2, 2, vec![[0.0; 3]; 3]),
            (u32::MAX, u32::MAX, vec![]),
            (257, 2, vec![]),
        ] {
            assert_eq!(
                screen(Some(ScreenSurface {
                    columns,
                    rows,
                    positions_m
                }))
                .geometry()
                .unwrap_err(),
                surface_error(ScreenSurfaceError::Dimensions)
            );
        }
        let mut surface = grid();
        if let Some(ScreenSurface { positions_m, .. }) = &mut surface {
            positions_m[4][2] = f64::NAN;
        }
        assert_eq!(
            screen(surface).geometry().unwrap_err(),
            surface_error(ScreenSurfaceError::NonFinite)
        );
    }

    #[test]
    fn collapsed_folded_and_globally_wrapped_grids_are_rejected() {
        for positions_m in [
            vec![
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
        ] {
            assert_eq!(
                screen(Some(ScreenSurface {
                    columns: 2,
                    rows: 2,
                    positions_m
                }))
                .geometry()
                .unwrap_err(),
                surface_error(ScreenSurfaceError::Folded)
            );
        }
        let positions_m = (0..2)
            .flat_map(|row| {
                (0..17).map(move |column| {
                    let angle = f64::from(column) * std::f64::consts::PI / 4.0;
                    let radius = 1.0 + f64::from(row);
                    [radius * angle.cos(), radius * angle.sin(), 0.0]
                })
            })
            .collect();
        assert_eq!(
            screen(Some(ScreenSurface {
                columns: 17,
                rows: 2,
                positions_m
            }))
            .geometry()
            .unwrap_err(),
            surface_error(ScreenSurfaceError::Folded)
        );
    }

    #[test]
    fn invalid_calibration_or_marker_identity_cannot_reach_queries() {
        let mut invalid = screen(None);
        invalid.calibration.homography = Matrix3::zeros();
        assert!(matches!(
            invalid.geometry(),
            Err(SurfaceError::Calibration(_))
        ));
        let mut invalid = screen(None);
        invalid.marker_ids[1] = invalid.marker_ids[0].clone();
        assert!(matches!(invalid.geometry(), Err(SurfaceError::Markers)));
        let mut invalid = screen(None);
        invalid.calibration.object_points[1] = invalid.calibration.object_points[0];
        assert!(matches!(invalid.geometry(), Err(SurfaceError::Markers)));
        let mut invalid = screen(None);
        invalid.calibration.object_points[0].x = f32::NAN;
        assert!(matches!(
            invalid.geometry(),
            Err(SurfaceError::Calibration(_))
        ));
    }
}
