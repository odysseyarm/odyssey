use std::{collections::HashMap, path::PathBuf};

use crate::{accessory::AccessoryInfo, hexkeymap::HexKeyMapN};
use app_dirs2::{get_app_root, AppDataType, AppInfo};
use ats_common::ScreenCalibration;
use serde::{Deserialize, Serialize};
use tokio::fs;

pub const APP_INFO: AppInfo = AppInfo {
    name: "odyssey",
    author: "odysseyarm",
};

/* ------------------------------- Versioning -------------------------------- */

// Current version for all config files
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionedConfig<T> {
    version: u32,
    #[serde(flatten)]
    data: T,
}

/* ------------------------- Corruption Event Types -------------------------- */

#[derive(Clone, Debug)]
pub struct ConfigCorruptionEvent {
    pub file_type: ConfigFileType,
    pub file_path: String,
    pub error_message: String,
    pub using_default: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum ConfigFileType {
    DeviceShotDelays,
    DeviceOffsets,
    ScreenCalibration,
    AccessoryMap,
}

/* ----------------------------- Load result type ---------------------------- */

#[derive(Debug, Clone)]
pub enum ConfigLoadResult<T> {
    Success(T),
    CorruptedUsingDefault { path: PathBuf, error: String },
    NotFoundUsingDefault,
}

impl<T> ConfigLoadResult<T> {
    pub fn into_value(self) -> T
    where
        T: Default,
    {
        match self {
            ConfigLoadResult::Success(val) => val,
            ConfigLoadResult::CorruptedUsingDefault { .. } => T::default(),
            ConfigLoadResult::NotFoundUsingDefault => T::default(),
        }
    }

    pub fn is_corrupted(&self) -> bool {
        matches!(self, ConfigLoadResult::CorruptedUsingDefault { .. })
    }

    pub fn corruption_info(&self) -> Option<(PathBuf, String)> {
        match self {
            ConfigLoadResult::CorruptedUsingDefault { path, error } => {
                Some((path.clone(), error.clone()))
            }
            _ => None,
        }
    }
}

/* ------------------------------- Shot delays ------------------------------- */

pub async fn device_shot_delays_async() -> ConfigLoadResult<HashMap<[u8; 6], u16>> {
    let config_dir = match get_app_root(AppDataType::UserConfig, &APP_INFO) {
        Ok(dir) => dir,
        Err(e) => {
            tracing::error!("Failed to get config directory: {}", e);
            return ConfigLoadResult::NotFoundUsingDefault;
        }
    };
    let path = config_dir.join("device_shot_delays.json");

    match fs::try_exists(&path).await {
        Ok(true) => {
            tracing::info!("Loading device delays from {}", path.display());
            match fs::read_to_string(&path).await {
                Ok(contents) => match json5::from_str::<HexKeyMapN<u16, 6>>(&contents) {
                    Ok(HexKeyMapN(map)) => ConfigLoadResult::Success(map),
                    Err(e) => {
                        tracing::error!(
                            "Corrupted device shot delays file at {}: {}. Using defaults.",
                            path.display(),
                            e
                        );
                        ConfigLoadResult::CorruptedUsingDefault {
                            path,
                            error: e.to_string(),
                        }
                    }
                },
                Err(e) => {
                    tracing::error!(
                        "Failed to read device shot delays file at {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    ConfigLoadResult::CorruptedUsingDefault {
                        path,
                        error: e.to_string(),
                    }
                }
            }
        }
        Ok(false) => {
            tracing::warn!("Device shot delays file not found, using default values");
            ConfigLoadResult::NotFoundUsingDefault
        }
        Err(e) => {
            tracing::error!("Failed to check if device shot delays file exists: {}", e);
            ConfigLoadResult::NotFoundUsingDefault
        }
    }
}

pub async fn device_shot_delays_save_async(
    device_shot_delays: &HashMap<[u8; 6], u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let path = config_dir.join("device_shot_delays.json");

    let wrapped = HexKeyMapN(device_shot_delays.clone());
    let contents = json5::to_string(&wrapped)?;
    fs::write(&path, contents).await?;
    Ok(())
}

pub async fn device_shot_delay_save_async(
    uuid: [u8; 6],
    delay: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut map = device_shot_delays_async().await.into_value();
    map.insert(uuid, delay);
    device_shot_delays_save_async(&map).await
}

/* --------------------------------- Offsets -------------------------------- */

pub async fn device_offsets_async() -> ConfigLoadResult<HashMap<[u8; 6], nalgebra::Isometry3<f32>>>
{
    let config_dir = match get_app_root(AppDataType::UserConfig, &APP_INFO) {
        Ok(dir) => dir,
        Err(e) => {
            tracing::error!("Failed to get config directory: {}", e);
            return ConfigLoadResult::NotFoundUsingDefault;
        }
    };
    let path = config_dir.join("device_offsets.json");

    match fs::try_exists(&path).await {
        Ok(true) => {
            tracing::info!("Loading device offsets from {}", path.display());
            match fs::read_to_string(&path).await {
                Ok(contents) => {
                    match json5::from_str::<HexKeyMapN<nalgebra::Isometry3<f32>, 6>>(&contents) {
                        Ok(HexKeyMapN(map)) => ConfigLoadResult::Success(map),
                        Err(e) => {
                            tracing::error!(
                                "Corrupted device offsets file at {}: {}. Using defaults.",
                                path.display(),
                                e
                            );
                            ConfigLoadResult::CorruptedUsingDefault {
                                path,
                                error: e.to_string(),
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to read device offsets file at {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    ConfigLoadResult::CorruptedUsingDefault {
                        path,
                        error: e.to_string(),
                    }
                }
            }
        }
        Ok(false) => {
            tracing::warn!("Device offsets file not found, using default values");
            ConfigLoadResult::NotFoundUsingDefault
        }
        Err(e) => {
            tracing::error!("Failed to check if device offsets file exists: {}", e);
            ConfigLoadResult::NotFoundUsingDefault
        }
    }
}

pub async fn device_offsets_save_async(
    device_offsets: &HashMap<[u8; 6], nalgebra::Isometry3<f32>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    let path = config_dir.join("device_offsets.json");

    let wrapped = HexKeyMapN(device_offsets.clone());
    let contents = json5::to_string(&wrapped)?;
    fs::write(&path, contents).await?;
    Ok(())
}

/* ---------------------------- Screen calibrations --------------------------- */

pub struct ScreenCalibrationsResult {
    pub calibrations: arrayvec::ArrayVec<
        (u8, ScreenCalibration<f32>),
        { (ats_common::MAX_SCREEN_ID + 1) as usize },
    >,
    pub corrupted_screens: Vec<(u8, PathBuf, String)>,
}

pub async fn screen_calibrations_async() -> ScreenCalibrationsResult {
    use arrayvec::ArrayVec;
    use tokio::fs;

    let config_dir = match get_app_root(AppDataType::UserConfig, &APP_INFO) {
        Ok(dir) => dir,
        Err(e) => {
            tracing::error!("Failed to get config directory: {}", e);
            return ScreenCalibrationsResult {
                calibrations: ArrayVec::new(),
                corrupted_screens: vec![],
            };
        }
    };

    let mut out = ArrayVec::<
        (u8, ScreenCalibration<f32>),
        { (ats_common::MAX_SCREEN_ID + 1) as usize },
    >::new();
    let mut corrupted_screens = Vec::new();

    for i in 0..((ats_common::MAX_SCREEN_ID + 1) as usize) {
        let path = config_dir
            .join("screens")
            .join(format!("screen_{}.json", i));
        match fs::try_exists(&path).await {
            Ok(true) => {
                tracing::info!("Loading screen calibration from {}", path.display());
                match fs::read_to_string(&path).await {
                    Ok(txt) => match json5::from_str::<ScreenCalibration<f32>>(&txt) {
                        Ok(cal) => {
                            let _ = out.try_push((i as u8, cal));
                        }
                        Err(e) => {
                            tracing::error!(
                                "Corrupted screen calibration for screen {}: {}. Ignoring.",
                                i,
                                e
                            );
                            corrupted_screens.push((i as u8, path, e.to_string()));
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            "Failed to read screen calibration for screen {}: {}. Ignoring.",
                            i,
                            e
                        );
                        corrupted_screens.push((i as u8, path, e.to_string()));
                    }
                }
            }
            Ok(false) => {
                // File doesn't exist, this is normal
            }
            Err(e) => {
                tracing::error!(
                    "Failed to check screen calibration file for screen {}: {}",
                    i,
                    e
                );
            }
        }
    }

    ScreenCalibrationsResult {
        calibrations: out,
        corrupted_screens,
    }
}

/* ------------------------------ Accessory map ------------------------------ */

type AccessoryMapWrap = HexKeyMapN<AccessoryInfo, 6>;

fn accessory_map_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    Ok(dir.join("accessory_map.json"))
}

pub async fn accessory_map_async() -> ConfigLoadResult<HashMap<[u8; 6], AccessoryInfo>> {
    let path = match accessory_map_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to get accessory map path: {}", e);
            return ConfigLoadResult::NotFoundUsingDefault;
        }
    };

    match fs::try_exists(&path).await {
        Ok(true) => match fs::read_to_string(&path).await {
            Ok(txt) => match json5::from_str::<AccessoryMapWrap>(&txt) {
                Ok(HexKeyMapN(wrapped)) => ConfigLoadResult::Success(wrapped.into_iter().collect()),
                Err(e) => {
                    tracing::error!(
                        "Corrupted accessory map file at {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    ConfigLoadResult::CorruptedUsingDefault {
                        path,
                        error: e.to_string(),
                    }
                }
            },
            Err(e) => {
                tracing::error!(
                    "Failed to read accessory map file at {}: {}. Using defaults.",
                    path.display(),
                    e
                );
                ConfigLoadResult::CorruptedUsingDefault {
                    path,
                    error: e.to_string(),
                }
            }
        },
        Ok(false) => {
            tracing::info!("Accessory map file not found, using default values");
            ConfigLoadResult::NotFoundUsingDefault
        }
        Err(e) => {
            tracing::error!("Failed to check if accessory map file exists: {}", e);
            ConfigLoadResult::NotFoundUsingDefault
        }
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
