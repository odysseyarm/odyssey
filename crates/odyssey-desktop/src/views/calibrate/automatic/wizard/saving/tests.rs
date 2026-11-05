use std::time::{SystemTime, UNIX_EPOCH};

use nalgebra::{Matrix3, Point3, UnitQuaternion};
use odyssey_autocalib::session::{load_calibration, save_calibration};
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::config::{self, MonitorKey};

use super::write_calibration;
use crate::ui_state::UiStateWriter;
use crate::views::calibrate::automatic::kind::DisplayKind;

fn monitor() -> MonitorKey {
    MonitorKey {
        name: Some("Save regression".into()),
        x: 0,
        y: 0,
        width: 1280,
        height: 720,
    }
}

fn calibration(scale: f32) -> ScreenCalibration<f32> {
    ScreenCalibration {
        surface: None,
        rotation: UnitQuaternion::identity(),
        homography: Matrix3::new(scale, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 1.0),
        object_points: [Point3::new(0.2, 0.3, 0.0); 6],
    }
}

async fn save_scenarios() {
    let dir = config::screens_dir().expect("simulation provides a screens directory");
    let original = calibration(0.5);
    save_calibration(&dir, 0, &original).expect("initial calibration is valid");
    let writer = UiStateWriter::start();

    let written = write_calibration(
        calibration(2.0),
        0,
        Some((monitor(), DisplayKind::Flat)),
        writer.clone(),
    )
    .await
    .expect("a valid calibration saves");
    assert_eq!(
        load_calibration(&dir, 0)
            .expect("saved calibration is readable")
            .homography,
        calibration(2.0).homography
    );
    assert!(written.screens[0], "the saved screen is reported as taken");
    assert!(written.path.contains(&dir.display().to_string()));
    let state = config::ui_state_async().await.into_value();
    assert_eq!(state.calibration_screen(&monitor()), Some(0));
    assert_eq!(state.display_kind(&monitor()), Some(DisplayKind::Flat));

    let invalid = ScreenCalibration {
        homography: Matrix3::zeros(),
        ..calibration(1.0)
    };
    write_calibration(invalid, 0, None, writer)
        .await
        .expect_err("an invalid calibration is rejected");
    assert_eq!(
        load_calibration(&dir, 0)
            .expect("a failed save keeps the readable calibration")
            .homography,
        calibration(2.0).homography
    );
}

#[test]
fn saving_replaces_a_screen_and_a_rejected_calibration_keeps_it() {
    const ROOT_ENV: &str = "ODYSSEY_CALIBRATION_SAVE_TEST_ROOT";
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        config::set_simulation_config_root(root.into()).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(save_scenarios());
        return;
    }

    // The simulated config root is process-global, so the scenario runs in a
    // child process that sets it before anything reads the config.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("odyssey-save-{}-{stamp}", std::process::id()));
    let test = concat!(
        module_path!(),
        "::saving_replaces_a_screen_and_a_rejected_calibration_keeps_it"
    );
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test.split_once("::").unwrap().1, "--nocapture"])
        .env(ROOT_ENV, &root)
        .status()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(status.success(), "isolated calibration save test failed");
}
