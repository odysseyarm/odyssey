use std::collections::HashMap;

use app_dirs2::{get_app_root, AppDataType, AppInfo};
use ats_cv::ScreenCalibration;
use crate::hexkeymap::HexKeyMap;

pub const APP_INFO: AppInfo = AppInfo {
    name: "odyssey",
    author: "odysseyarm",
};

pub fn device_offsets() -> Result<HashMap<u64, nalgebra::Isometry3<f32>>, Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let device_offsets_path = config_dir.join("device_offsets.json");

    if device_offsets_path.exists() {
        tracing::info!("Loading device offsets from {}", device_offsets_path.display());
        let contents = std::fs::read_to_string(&device_offsets_path)?;
        let HexKeyMap(map) = json5::from_str::<HexKeyMap<nalgebra::Isometry3<f32>>>(&contents)?;
        Ok(map)
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
                tracing::info!("Loading screen calibration from {}", screen_path.display());
                match json5::from_str(&std::fs::read_to_string(&screen_path).ok()?) {
                    Ok(calibration) => Some((i as u8, calibration)),
                    Err(e) => {
                        tracing::error!("Failed to parse screen calibration for screen {}: {}", i, e);
                        None
                    }
                }
            } else {
                None
            }
        })
        .collect())
}

pub fn save_device_offsets(
    device_offsets: &HashMap<u64, nalgebra::Isometry3<f32>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let device_offsets_path = config_dir.join("device_offsets.json");

    let converted: HashMap<String, &nalgebra::Isometry3<f32>> = device_offsets
        .iter()
        .map(|(k, v)| {
            let key_str = format!("0x{:02x}", k);
            (key_str, v)
        })
        .collect();

    std::fs::write(device_offsets_path, json5::to_string(&converted)?)?;
    Ok(())
}
