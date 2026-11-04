use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use nalgebra::Isometry3;
use odyssey_hub_common::accessory::AccessoryInfoMap;
use odyssey_hub_common::config::{
    ConfigCorruptionEvent, ConfigFileType, ConfigLoadResult, accessory_map_async,
    device_offsets_async, device_shot_delays_async, screen_calibrations_async, screens_dir,
};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::screen_watch::{Calibrations, ScreenCalibrationWatcher};

/// What the hub reads from the user's config files at startup.
///
/// A corrupted file never stops the hub: its default is used and the corruption
/// is reported in `corruptions`, which the app shows to the user.
pub(crate) struct StartupConfig {
    pub screen_calibrations: Arc<ArcSwap<Calibrations>>,
    pub device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    pub device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>,
    pub accessory_info: AccessoryInfoMap,
    pub corruptions: Vec<ConfigCorruptionEvent>,
}

impl StartupConfig {
    /// Loads every config file. Calibrations written while the hub runs (wizard,
    /// hand edits) apply without a restart: a watcher task keeps `screen_calibrations`
    /// current until `screen_watch_cancel` fires.
    pub(crate) async fn load(screen_watch_cancel: CancellationToken) -> Self {
        let mut corruptions = Vec::new();

        // The watcher snapshots the directory first so a write during the initial
        // load is seen as a change afterwards.
        let screen_watcher = match screens_dir() {
            Ok(dir) => Some(ScreenCalibrationWatcher::new(dir).await),
            Err(e) => {
                tracing::warn!("Screen calibrations will not hot reload: {}", e);
                None
            }
        };
        let screen_result = screen_calibrations_async().await;
        for (screen_id, path, error) in &screen_result.corrupted_screens {
            tracing::error!("Screen {} calibration corrupted: {}", screen_id, error);
            corruptions.push(corruption_event(
                ConfigFileType::ScreenCalibration,
                path,
                error.clone(),
            ));
        }
        let screen_calibrations = Arc::new(ArcSwap::from(Arc::new(screen_result.calibrations)));
        tracing::info!("Screen calibrations loaded: {screen_calibrations:?}");

        if let Some(watcher) = screen_watcher {
            // Best effort: the watcher cannot fail, and the caller's cancel stops it.
            tokio::spawn(watcher.run(Arc::clone(&screen_calibrations), screen_watch_cancel));
        }

        let device_offsets = take_value(
            device_offsets_async().await,
            ConfigFileType::DeviceOffsets,
            "Device offsets",
            &mut corruptions,
        );
        let device_shot_delays = take_value(
            device_shot_delays_async().await,
            ConfigFileType::DeviceShotDelays,
            "Device shot delays",
            &mut corruptions,
        );
        let accessory_info = take_value(
            accessory_map_async().await,
            ConfigFileType::AccessoryMap,
            "Accessory map",
            &mut corruptions,
        );

        Self {
            screen_calibrations,
            device_offsets: Arc::new(Mutex::new(device_offsets)),
            device_shot_delays: Arc::new(RwLock::new(device_shot_delays)),
            accessory_info,
            corruptions,
        }
    }
}

/// Unwraps a loaded config, recording a corrupted file (whose default is used).
fn take_value<T: Default>(
    result: ConfigLoadResult<T>,
    file_type: ConfigFileType,
    description: &str,
    corruptions: &mut Vec<ConfigCorruptionEvent>,
) -> T {
    if let Some((path, error)) = result.corruption_info() {
        tracing::error!("{description} corrupted: {error}");
        corruptions.push(corruption_event(file_type, &path, error));
    }
    result.into_value()
}

fn corruption_event(
    file_type: ConfigFileType,
    path: &Path,
    error_message: String,
) -> ConfigCorruptionEvent {
    ConfigCorruptionEvent {
        file_type,
        file_path: path.display().to_string(),
        error_message,
        using_default: true,
    }
}
