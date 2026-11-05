//! Firmware update UI for individual devices.
//!
//! Update state lives in a shared `Arc<Mutex<HashMap>>`. The update itself runs
//! on the tokio runtime, outside any view's lifetime, and views redraw when the
//! manager's ticker fires. Its claim follows the future through cancellation;
//! task panics become failures rather than leaving a device claimed forever.
//! Every image must be nonempty and match its manifest size before DFU starts.

mod bootloader;
mod device_firmware_update;
mod dfu_session;
mod downloads;
mod firmware_channel;
mod netcore;
mod poll_window;
mod update_available;
mod update_check;
mod update_manager;
mod update_notes;
mod update_state;
mod update_task;
mod updating_device_row;

pub(crate) use device_firmware_update::device_firmware_update;
pub(crate) use firmware_channel::FirmwareChannelModel;
pub(crate) use update_manager::{FirmwareUpdateManager, UpdateTicker};
pub(crate) use updating_device_row::updating_device_row;
