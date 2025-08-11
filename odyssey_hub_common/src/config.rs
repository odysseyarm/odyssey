use std::{collections::HashMap, path::PathBuf};

use crate::{hexkeymap::{HexKeyMap, HexKeyMapN}, AccessoryInfo};
use app_dirs2::{get_app_root, AppDataType, AppInfo};
use ats_cv::ScreenCalibration;

pub const APP_INFO: AppInfo = AppInfo {
    name: "odyssey",
    author: "odysseyarm",
};

pub fn device_shot_delays() -> Result<HashMap<u64, u16>, Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let device_shot_delays_path = config_dir.join("device_shot_delays.json");

    if device_shot_delays_path.exists() {
        tracing::info!(
            "Loading device delays from {}",
            device_shot_delays_path.display()
        );
        let contents = std::fs::read_to_string(&device_shot_delays_path)?;
        let HexKeyMap(map) = json5::from_str::<HexKeyMap<u16>>(&contents)?;
        Ok(map)
    } else {
        tracing::warn!("Device shot delays file not found, using default values");
        Ok(Default::default())
    }
}

pub fn device_offsets() -> Result<HashMap<u64, nalgebra::Isometry3<f32>>, Box<dyn std::error::Error>>
{
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let device_offsets_path = config_dir.join("device_offsets.json");

    if device_offsets_path.exists() {
        tracing::info!(
            "Loading device offsets from {}",
            device_offsets_path.display()
        );
        let contents = std::fs::read_to_string(&device_offsets_path)?;
        let HexKeyMap(map) = json5::from_str::<HexKeyMap<nalgebra::Isometry3<f32>>>(&contents)?;
        Ok(map)
    } else {
        tracing::warn!("Device offsets file not found, using default values");
        Ok(Default::default())
    }
}

pub fn screen_calibrations() -> Result<
    arrayvec::ArrayVec<
        (u8, ScreenCalibration<f32>),
        { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
    >,
    Box<dyn std::error::Error>,
> {
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
                        tracing::error!(
                            "Failed to parse screen calibration for screen {}: {}",
                            i,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            }
        })
        .collect())
}

pub fn device_shot_delays_save(
    device_shot_delays: &HashMap<u64, u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let device_shot_delays_path = config_dir.join("device_shot_delays.json");

    let converted: HashMap<String, u16> = device_shot_delays
        .iter()
        .map(|(k, v)| {
            let key_str = format!("0x{:02x}", k);
            (key_str, *v)
        })
        .collect();

    std::fs::write(device_shot_delays_path, json5::to_string(&converted)?)?;
    Ok(())
}

pub fn device_shot_delay_save(uuid: u64, delay: u16) -> Result<(), Box<dyn std::error::Error>> {
    let mut map = device_shot_delays()?;
    map.insert(uuid, delay);
    device_shot_delays_save(&map)
}

pub fn device_offsets_save(
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

type AccessoryMap = HexKeyMapN<AccessoryInfo, 6>;

fn accessory_map_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    Ok(dir.join("accessory_map.json"))
}

pub fn accessory_map() -> Result<HashMap<[u8;6], AccessoryInfo>, Box<dyn std::error::Error>> {
    let path = accessory_map_path()?;
    if path.exists() {
        let txt = std::fs::read_to_string(&path)?;
        let HexKeyMapN(wrapped): AccessoryMap = json5::from_str(&txt)?;
        Ok(wrapped.into_iter()
            .map(|(k, v)| (k, v))
            .collect())
    } else {
        Ok(Default::default())
    }
}

pub fn accessory_map_save(map: &HashMap<[u8; 6], AccessoryInfo>) -> Result<(), Box<dyn std::error::Error>> {
    let path = accessory_map_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let wrapped = HexKeyMapN(map.clone());

    let contents = json5::to_string(&wrapped)?;
    std::fs::write(&path, contents)?;

    Ok(())
}
