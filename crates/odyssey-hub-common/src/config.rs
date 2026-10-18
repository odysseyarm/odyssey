use std::collections::HashMap;
use std::path::{Path, PathBuf};

use app_dirs2::{AppDataType, AppInfo, get_app_root};
use odyssey_common::ScreenCalibration;
use tokio::fs;

use crate::accessory::AccessoryInfo;
use crate::hexkeymap::HexKeyMapN;

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
/// identical panels share a name), so the key also carries the monitor's position
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
    /// The screen (`screen_<id>.json`) the calibration wizard last saved for
    /// each monitor, so the next calibration of that monitor starts on the
    /// same screen number.
    pub calibration_screens: Vec<MonitorScreen>,
    /// What kind of display the calibration wizard last calibrated on each
    /// monitor, so a projector is not asked about again as a TV.
    pub display_kinds: Vec<MonitorDisplayKind>,
}

/// What a display is, as far as calibration cares: a flat panel has a known
/// physical size, a projected picture does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayKind {
    /// A TV or monitor.
    #[default]
    Flat,
    /// A projector, or any projected image.
    Projector,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MonitorDisplayKind {
    pub monitor: MonitorKey,
    pub kind: DisplayKind,
}

/// A monitor and the screen (`screen_<id>.json`) calibrated for it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MonitorScreen {
    pub monitor: MonitorKey,
    pub screen_id: u8,
}

impl UiState {
    /// The screen id last calibrated for `monitor`, if any.
    pub fn calibration_screen(&self, monitor: &MonitorKey) -> Option<u8> {
        self.calibration_screens
            .iter()
            .find(|s| &s.monitor == monitor)
            .map(|s| s.screen_id)
    }

    /// Records that `screen_id` now holds `monitor`'s calibration. A screen id
    /// holds one screen, so any other monitor that pointed at it loses the
    /// claim.
    pub fn remember_calibration_screen(&mut self, monitor: MonitorKey, screen_id: u8) {
        self.calibration_screens
            .retain(|s| s.monitor != monitor && s.screen_id != screen_id);
        self.calibration_screens
            .push(MonitorScreen { monitor, screen_id });
    }

    /// The kind of display last calibrated on `monitor`, if any.
    pub fn display_kind(&self, monitor: &MonitorKey) -> Option<DisplayKind> {
        self.display_kinds
            .iter()
            .find(|k| &k.monitor == monitor)
            .map(|k| k.kind)
    }

    pub fn remember_display_kind(&mut self, monitor: MonitorKey, kind: DisplayKind) {
        self.display_kinds.retain(|k| k.monitor != monitor);
        self.display_kinds
            .push(MonitorDisplayKind { monitor, kind });
    }
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

/// Directory holding `screen_<id>.json`, one file per calibrated screen.
pub fn screens_dir() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    Ok(get_app_root(AppDataType::UserConfig, &APP_INFO)?.join("screens"))
}

/// File name of the calibration for screen `id` inside [`screens_dir`].
pub fn screen_file_name(id: u8) -> String {
    format!("screen_{id}.json")
}

pub struct ScreenCalibrationsResult {
    pub calibrations: arrayvec::ArrayVec<
        (u8, ScreenCalibration<f32>),
        { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
    >,
    pub corrupted_screens: Vec<(u8, PathBuf, String)>,
}

/// Loads every `screen_<id>.json` in the user's screens directory.
pub async fn screen_calibrations_async() -> ScreenCalibrationsResult {
    match screens_dir() {
        Ok(dir) => screen_calibrations_from_dir_async(&dir).await,
        Err(e) => {
            tracing::error!("Failed to get config directory: {}", e);
            ScreenCalibrationsResult {
                calibrations: arrayvec::ArrayVec::new(),
                corrupted_screens: vec![],
            }
        }
    }
}

/// Loads every `screen_<id>.json` in `dir`, ordered by id. A file that is
/// unreadable or does not parse is reported in `corrupted_screens` and left out
/// of `calibrations`; a missing file is simply absent.
pub async fn screen_calibrations_from_dir_async(dir: &Path) -> ScreenCalibrationsResult {
    let mut calibrations = arrayvec::ArrayVec::new();
    let mut corrupted_screens = Vec::new();

    for i in 0..=odyssey_common::MAX_SCREEN_ID {
        let path = dir.join(screen_file_name(i));
        let txt = match fs::read_to_string(&path).await {
            Ok(txt) => txt,
            // A file that vanishes between listing and reading counts as absent.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::error!(
                    "Failed to read screen calibration for screen {}: {}. Ignoring.",
                    i,
                    e
                );
                corrupted_screens.push((i, path, e.to_string()));
                continue;
            }
        };
        tracing::info!("Loading screen calibration from {}", path.display());
        match json5::from_str::<ScreenCalibration<f32>>(&txt) {
            Ok(cal) => {
                let _ = calibrations.try_push((i, cal));
            }
            Err(e) => {
                tracing::error!(
                    "Corrupted screen calibration for screen {}: {}. Ignoring.",
                    i,
                    e
                );
                corrupted_screens.push((i, path, e.to_string()));
            }
        }
    }

    ScreenCalibrationsResult {
        calibrations,
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
            calibration_screens: vec![MonitorScreen {
                monitor: key(),
                screen_id: 3,
            }],
            display_kinds: vec![MonitorDisplayKind {
                monitor: key(),
                kind: DisplayKind::Projector,
            }],
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

    fn other_key() -> MonitorKey {
        MonitorKey {
            x: 0,
            name: Some("LG 27GP950".into()),
            ..key()
        }
    }

    #[test]
    fn remembered_screen_is_per_monitor_and_replaced_on_recalibration() {
        let mut state = UiState::default();
        assert_eq!(state.calibration_screen(&key()), None);

        state.remember_calibration_screen(key(), 2);
        state.remember_calibration_screen(other_key(), 4);
        assert_eq!(state.calibration_screen(&key()), Some(2));
        assert_eq!(state.calibration_screen(&other_key()), Some(4));

        state.remember_calibration_screen(key(), 1);
        assert_eq!(state.calibration_screen(&key()), Some(1));
        assert_eq!(state.calibration_screens.len(), 2);
    }

    #[test]
    fn saving_over_a_screen_takes_it_from_the_monitor_that_had_it() {
        let mut state = UiState::default();
        state.remember_calibration_screen(key(), 2);
        state.remember_calibration_screen(other_key(), 2);
        assert_eq!(state.calibration_screen(&key()), None);
        assert_eq!(state.calibration_screen(&other_key()), Some(2));
    }

    #[test]
    fn display_kind_is_remembered_per_monitor_and_replaced() {
        let mut state = UiState::default();
        assert_eq!(state.display_kind(&key()), None);
        state.remember_display_kind(key(), DisplayKind::Projector);
        state.remember_display_kind(other_key(), DisplayKind::Flat);
        assert_eq!(state.display_kind(&key()), Some(DisplayKind::Projector));
        assert_eq!(state.display_kind(&other_key()), Some(DisplayKind::Flat));
        state.remember_display_kind(key(), DisplayKind::Flat);
        assert_eq!(state.display_kind(&key()), Some(DisplayKind::Flat));
        assert_eq!(state.display_kinds.len(), 2);
    }

    #[test]
    fn display_kind_is_written_in_plain_words() {
        let json = serde_json::to_string(&DisplayKind::Projector).unwrap();
        assert_eq!(json, "\"projector\"");
        assert_eq!(
            json5::from_str::<DisplayKind>("\"flat\"").unwrap(),
            DisplayKind::Flat
        );
    }

    #[test]
    fn ui_state_written_before_screens_existed_still_loads() {
        let state: UiState = json5::from_str(
            r#"{ "selected_monitor": { "name": null, "x": 0, "y": 0, "width": 1920, "height": 1080 } }"#,
        )
        .unwrap();
        assert!(state.selected_monitor.is_some());
        assert!(state.calibration_screens.is_empty());
        assert!(state.display_kinds.is_empty());
    }
}
