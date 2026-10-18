use std::{collections::HashMap, path::PathBuf};

use crate::{accessory::AccessoryInfo, hexkeymap::HexKeyMapN};
use app_dirs2::{get_app_root, AppDataType, AppInfo};
use odyssey_common::ScreenCalibration;
use tokio::fs;

pub const APP_INFO: AppInfo = AppInfo {
    name: "odyssey",
    author: "odysseyarm",
};

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

/* ---------------------------- Firmware channel ----------------------------- */

/// The firmware channel the user chose, by name. `None` when they never chose
/// one or the file cannot be read, which callers treat as the default.
pub async fn firmware_channel_async() -> Option<String> {
    let path = get_app_root(AppDataType::UserConfig, &APP_INFO)
        .ok()?
        .join("firmware_channel");
    let name = fs::read_to_string(&path).await.ok()?;
    Some(name.trim().to_string())
}

pub async fn firmware_channel_save_async(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_app_root(AppDataType::UserConfig, &APP_INFO)?;
    fs::write(config_dir.join("firmware_channel"), name).await?;
    Ok(())
}

/* -------------------------------- UI state --------------------------------- */

/// Identifies a monitor across launches. Models repeat across a desk (two
/// identical panels share a name), so the key also carries the monitor's slot
/// in the virtual desktop. Rearranging the monitors changes the key, which is
/// the right outcome: the remembered pick no longer names the same screen.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MonitorKey {
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Desktop app state that is not a setting but should survive a restart.
///
/// Every field defaults when absent, so files written by older versions keep
/// loading after new fields land.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiState {
    /// The monitor the user last picked in the screen picker.
    pub selected_monitor: Option<MonitorKey>,
}

fn ui_state_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(get_app_root(AppDataType::UserConfig, &APP_INFO)?.join("ui_state.json"))
}

pub async fn ui_state_async() -> ConfigLoadResult<UiState> {
    match ui_state_path() {
        Ok(path) => load_ui_state_from(path).await,
        Err(e) => {
            tracing::error!("Failed to get UI state path: {}", e);
            ConfigLoadResult::NotFoundUsingDefault
        }
    }
}

pub async fn ui_state_save_async(state: &UiState) -> Result<(), Box<dyn std::error::Error>> {
    save_ui_state_to(&ui_state_path()?, state).await
}

async fn load_ui_state_from(path: PathBuf) -> ConfigLoadResult<UiState> {
    let contents = match fs::read_to_string(&path).await {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("UI state file not found, using default values");
            return ConfigLoadResult::NotFoundUsingDefault;
        }
        Err(e) => {
            tracing::error!(
                "Failed to read UI state file at {}: {}. Using defaults.",
                path.display(),
                e
            );
            return ConfigLoadResult::CorruptedUsingDefault {
                path,
                error: e.to_string(),
            };
        }
    };

    match json5::from_str::<UiState>(&contents) {
        Ok(state) => ConfigLoadResult::Success(state),
        Err(e) => {
            tracing::error!(
                "Corrupted UI state file at {}: {}. Using defaults.",
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

async fn save_ui_state_to(
    path: &std::path::Path,
    state: &UiState,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(path, serde_json::to_string_pretty(state)?).await?;
    Ok(())
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
        { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
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
        { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
    >::new();
    let mut corrupted_screens = Vec::new();

    for i in 0..((odyssey_common::MAX_SCREEN_ID + 1) as usize) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn key() -> MonitorKey {
        MonitorKey {
            name: Some("DELL U2723QE".into()),
            x: -2560,
            y: 0,
            width: 2560,
            height: 1440,
        }
    }

    #[test]
    fn ui_state_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("odyssey-ui-state-rt-{}", std::process::id()));
        let path = dir.join("nested").join("ui_state.json");
        let state = UiState {
            selected_monitor: Some(key()),
        };

        block_on(async {
            save_ui_state_to(&path, &state).await.unwrap();
            match load_ui_state_from(path.clone()).await {
                ConfigLoadResult::Success(loaded) => assert_eq!(loaded, state),
                other => panic!("expected Success, got {other:?}"),
            }
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ui_state_missing_file_uses_default() {
        let path = std::env::temp_dir().join("odyssey-ui-state-missing/ui_state.json");
        let result = block_on(load_ui_state_from(path));
        assert!(matches!(result, ConfigLoadResult::NotFoundUsingDefault));
        assert_eq!(result.into_value(), UiState::default());
    }

    #[test]
    fn ui_state_corrupt_file_falls_back_to_default() {
        let dir = std::env::temp_dir().join(format!("odyssey-ui-state-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui_state.json");
        std::fs::write(&path, "{ selected_monitor: [oops").unwrap();

        let result = block_on(load_ui_state_from(path));
        assert!(result.is_corrupted());
        assert_eq!(result.into_value(), UiState::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ui_state_tolerates_missing_and_unknown_fields() {
        assert_eq!(
            json5::from_str::<UiState>("{}").unwrap(),
            UiState::default()
        );
        let state: UiState = json5::from_str(r#"{ "from_a_newer_version": 3 }"#).unwrap();
        assert_eq!(state, UiState::default());
    }
}
