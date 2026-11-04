//! Reading and writing the calibration files the hub loads.

use std::io;
use std::path::{Path, PathBuf};

use odyssey_common::{MAX_SCREEN_ID, ScreenCalibration};

/// File name the hub reads for screen `id`.
pub fn screen_file_name(id: u8) -> String {
    format!("screen_{id}.json")
}

/// Serializes a calibration the way the hub's loader expects: compact JSON
/// with `rotation` as `[i, j, k, w]`, `homography` as nine column-major
/// values and `object_points` as six `[x, y, z]` triples.
pub fn calibration_to_json(
    calibration: &ScreenCalibration<f32>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(calibration)
}

pub fn calibration_from_json(json: &str) -> Result<ScreenCalibration<f32>, serde_json::Error> {
    serde_json::from_str(json)
}

fn check_id(id: u8) -> io::Result<()> {
    if id > MAX_SCREEN_ID {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("screen id {id} is above {MAX_SCREEN_ID}"),
        ));
    }
    Ok(())
}

/// Writes `screen_<id>.json` into `screens_dir`, creating the directory. The
/// write goes through a temporary file so the hub never reads a partial
/// calibration. Returns the file's path.
pub fn save_calibration(
    screens_dir: &Path,
    id: u8,
    calibration: &ScreenCalibration<f32>,
) -> io::Result<PathBuf> {
    check_id(id)?;
    std::fs::create_dir_all(screens_dir)?;
    let path = screens_dir.join(screen_file_name(id));
    let temp = screens_dir.join(format!(".{}.tmp", screen_file_name(id)));
    let json = calibration_to_json(calibration).map_err(io::Error::other)?;
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &path)?;
    Ok(path)
}

pub fn load_calibration(screens_dir: &Path, id: u8) -> io::Result<ScreenCalibration<f32>> {
    check_id(id)?;
    let json = std::fs::read_to_string(screens_dir.join(screen_file_name(id)))?;
    calibration_from_json(&json).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use nalgebra::{Matrix3, Point2, Point3, UnitQuaternion};

    use super::*;

    fn sample() -> ScreenCalibration<f32> {
        ScreenCalibration {
            rotation: UnitQuaternion::from_euler_angles(0.1, -0.2, 0.05),
            homography: Matrix3::new(0.82, 0.0, 0.0, 0.0, 1.46, 0.0, 0.0, 0.0, 1.0),
            object_points: [
                Point3::new(0.21, 0.2, 0.0),
                Point3::new(0.18, 0.56, 0.0),
                Point3::new(0.93, 0.55, 0.0),
                Point3::new(0.62, 0.24, 0.0),
                Point3::new(0.96, 0.17, 0.0),
                Point3::new(0.6, 0.52, 0.0),
            ],
        }
    }

    #[test]
    fn json_round_trip_keeps_corners_and_markers() {
        let cal = sample();
        let json = calibration_to_json(&cal).unwrap();
        let back = calibration_from_json(&json).unwrap();
        assert_eq!(back.homography, cal.homography);
        assert_eq!(back.object_points, cal.object_points);
        assert!((back.rotation.angle_to(&cal.rotation)).abs() < 1e-6);
        let expected = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0 / 0.82, 0.0),
            Point2::new(0.0, 1.0 / 1.46),
            Point2::new(1.0 / 0.82, 1.0 / 1.46),
        ];
        for (got, want) in back.bounds().iter().zip(expected) {
            assert!((got - want).norm() < 1e-6, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn json_layout_matches_the_hub_schema() {
        let value: serde_json::Value =
            serde_json::from_str(&calibration_to_json(&sample()).unwrap()).unwrap();
        assert_eq!(value["rotation"].as_array().unwrap().len(), 4);
        assert_eq!(value["homography"].as_array().unwrap().len(), 9);
        assert_eq!(value["object_points"].as_array().unwrap().len(), 6);
        // Column-major: the second entry is the (row 1, col 0) element, zero here,
        // and the 5th entry is the (1, 1) element.
        assert!((value["homography"][4].as_f64().unwrap() - 1.46).abs() < 1e-6);
        assert!((value["homography"][0].as_f64().unwrap() - 0.82).abs() < 1e-6);
    }

    #[test]
    fn save_and_load_use_the_hub_file_name() {
        let dir =
            std::env::temp_dir().join(format!("odyssey-autocalib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_calibration(&dir.join("screens"), 3, &sample()).unwrap();
        assert_eq!(path.file_name().unwrap(), "screen_3.json");
        let back = load_calibration(&dir.join("screens"), 3).unwrap();
        assert_eq!(back.object_points, sample().object_points);
        assert!(save_calibration(&dir, MAX_SCREEN_ID + 1, &sample()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
