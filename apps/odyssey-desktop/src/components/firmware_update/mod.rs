//! Firmware update UI components for individual devices.
//!
//! Architecture:
//! - Firmware update state is stored in a shared `Arc<Mutex<HashMap>>` between
//!   the tokio runtime and Dioxus components
//! - The actual update runs entirely on `tokio::spawn`, independent of Dioxus component lifecycle
//! - Dioxus components poll the shared state periodically to update the UI

mod bootloader;
mod device_firmware_update;
mod dfu_steps;
mod firmware_channel;
mod netcore;
mod update_check;
mod update_manager;
mod update_notes;
mod update_state;
mod update_task;
mod updating_device_row;

pub(crate) use device_firmware_update::DeviceFirmwareUpdate;
pub(crate) use firmware_channel::{FIRMWARE_CHANNEL, ensure_firmware_channel_loaded};
pub(crate) use update_manager::FirmwareUpdateManager;
pub(crate) use updating_device_row::UpdatingDeviceRow;
