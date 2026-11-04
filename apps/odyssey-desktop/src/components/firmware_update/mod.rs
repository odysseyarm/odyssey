//! Firmware update UI for individual devices.
//!
//! Update state lives in a shared `Arc<Mutex<HashMap>>`. The update itself runs
//! on `tokio::spawn`, outside any component's lifetime, and components poll the
//! shared state to redraw.

mod bootloader;
mod device_firmware_update;
mod dfu_session;
mod downloads;
mod firmware_channel;
mod netcore;
mod progress_bar;
mod update_available;
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
