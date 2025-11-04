use std::{collections::HashMap, path::PathBuf};

use crate::{
    accessory::AccessoryInfo,
    hexkeymap::{HexKeyMap, HexKeyMapN},
};
use app_dirs2::{get_app_root, AppDataType, AppInfo};
use ats_common::ScreenCalibration;
use tokio::fs;

pub const APP_INFO: AppInfo = AppInfo {
    name: "odyssey",
    author: "odysseyarm",
};

/* ------------------------------- Shot delays ------------------------------- */

pub async fn device_shot_delays_async() -> Result<HashMap<u64, u16>, Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let path = config_dir.join("device_shot_delays.json");

    if fs::try_exists(&path).await? {
        tracing::info!("Loading device delays from {}", path.display());
        let contents = fs::read_to_string(&path).await?;
        let HexKeyMap(map) = json5::from_str::<HexKeyMap<u16>>(&contents)?;
        Ok(map)
    } else {
        tracing::warn!("Device shot delays file not found, using default values");
        Ok(Default::default())
    }
}

pub async fn device_shot_delays_save_async(
    device_shot_delays: &HashMap<u64, u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let path = config_dir.join("device_shot_delays.json");

    // Keep the existing string-key format (e.g., "0x..")
    let converted: HashMap<String, u16> = device_shot_delays
        .iter()
        .map(|(k, v)| (format!("0x{:02x}", k), *v))
        .collect();

    fs::write(path, json5::to_string(&converted)?).await?;
    Ok(())
}

pub async fn device_shot_delay_save_async(
    uuid: u64,
    delay: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut map = device_shot_delays_async().await?;
    map.insert(uuid, delay);
    device_shot_delays_save_async(&map).await
}

/* --------------------------------- Offsets -------------------------------- */

pub async fn device_offsets_async(
) -> Result<HashMap<u64, nalgebra::Isometry3<f32>>, Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let path = config_dir.join("device_offsets.json");

    if fs::try_exists(&path).await? {
        tracing::info!("Loading device offsets from {}", path.display());
        let contents = fs::read_to_string(&path).await?;
        let HexKeyMap(map) = json5::from_str::<HexKeyMap<nalgebra::Isometry3<f32>>>(&contents)?;
        Ok(map)
    } else {
        tracing::warn!("Device offsets file not found, using default values");
        Ok(Default::default())
    }
}

pub async fn device_offsets_save_async(
    device_offsets: &HashMap<u64, nalgebra::Isometry3<f32>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let path = config_dir.join("device_offsets.json");

    // Preserve current format: stringified keys
    let converted: HashMap<String, &nalgebra::Isometry3<f32>> = device_offsets
        .iter()
        .map(|(k, v)| (format!("0x{:02x}", k), v))
        .collect();

    fs::write(path, json5::to_string(&converted)?).await?;
    Ok(())
}

/* ---------------------------- Screen calibrations --------------------------- */

pub async fn screen_calibrations_async() -> Result<
    arrayvec::ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
    Box<dyn std::error::Error>,
> {
    use arrayvec::ArrayVec;
    use tokio::fs;

    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let mut out = ArrayVec::<
        (u8, ScreenCalibration<f32>),
        { (ats_common::MAX_SCREEN_ID + 1) as usize },
    >::new();

    for i in 0..((ats_common::MAX_SCREEN_ID + 1) as usize) {
        let path = config_dir
            .join("screens")
            .join(format!("screen_{}.json", i));
        if fs::try_exists(&path).await? {
            tracing::info!("Loading screen calibration from {}", path.display());
            let txt = fs::read_to_string(&path).await?;
            match json5::from_str::<ScreenCalibration<f32>>(&txt) {
                Ok(cal) => {
                    // ✅ push the tuple
                    let _ = out.try_push((i as u8, cal));
                }
                Err(e) => {
                    tracing::error!("Failed to parse screen calibration for screen {}: {}", i, e);
                }
            }
        }
    }

    Ok(out)
}

/* ------------------------------ Accessory map ------------------------------ */

type AccessoryMapWrap = HexKeyMapN<AccessoryInfo, 6>;

fn accessory_map_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    Ok(dir.join("accessory_map.json"))
}

pub async fn accessory_map_async(
) -> Result<HashMap<[u8; 6], AccessoryInfo>, Box<dyn std::error::Error>> {
    let path = accessory_map_path()?;
    if fs::try_exists(&path).await? {
        let txt = fs::read_to_string(&path).await?;
        let HexKeyMapN(wrapped): AccessoryMapWrap = json5::from_str(&txt)?;
        Ok(wrapped.into_iter().collect())
    } else {
        Ok(Default::default())
    }
}

pub async fn accessory_map_save_async(
    map: &HashMap<[u8; 6], AccessoryInfo>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = accessory_map_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let wrapped = HexKeyMapN(map.clone());
    let contents = json5::to_string(&wrapped)?;
    fs::write(&path, contents).await?;
    Ok(())
}
