use std::{collections::HashMap, fs::File};

use app_dirs2::{get_app_root, AppDataType, AppInfo};
use ats_cv::ScreenCalibration;

use tracing::error;

pub const APP_INFO: AppInfo = AppInfo {
    name: "odyssey",
    author: "odysseyarm",
};

pub fn device_offsets() -> Result<HashMap<[u8; 6], nalgebra::Isometry3<f32>>, Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;

    let device_offsets_path = config_dir.join("device_offsets.json");
    if device_offsets_path.exists() {
        Ok(serde_json::from_reader(File::open(device_offsets_path)?)?)
    } else {
        tracing::warn!("Device offsets file not found, using default values");
        Ok(Default::default())
    }
}

pub fn screen_calibrations() -> Result<arrayvec::ArrayVec<(u8, ScreenCalibration<f32>), { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize }>, Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    Ok((0..{ (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize })
        .filter_map(|i| {
            let screen_path = config_dir
                .join("screens")
                .join(std::format!("screen_{}.json", i));
            if screen_path.exists() {
                File::open(screen_path)
                    .ok()
                    .and_then(|file| match serde_json::from_reader(file) {
                        Ok(calibration) => Some((i as u8, calibration)),
                        Err(e) => {
                            error!("Failed to deserialize screen calibration: {}", e);
                            None
                        }
                    })
            } else {
                None
            }
        })
        .collect())
}

pub fn save_device_offsets(device_offsets: &HashMap<[u8; 6], nalgebra::Isometry3<f32>>) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let device_offsets_path = config_dir.join("device_offsets.json");
    let file = File::create(device_offsets_path)?;
    serde_json::to_writer_pretty(file, device_offsets)?;
    Ok(())
}
