#![expect(
    unused_qualifications,
    reason = "dioxus-rsx expands event attributes such as `onclick:` into paths the lint \
              flags at the user's span; they cannot be shortened in the markup"
)]

mod config_corruption_modal;
pub(crate) use config_corruption_modal::ConfigCorruptionModal;

mod navbar;
pub(crate) use navbar::Navbar;

mod page;
pub(crate) use page::Page;

mod theme;
pub(crate) use theme::Theme;

pub(crate) mod crosshair_manager;

pub(crate) mod firmware_update;
pub(crate) use firmware_update::{DeviceFirmwareUpdate, FirmwareUpdateManager, UpdatingDeviceRow};

pub(crate) mod sensor_settings;
pub(crate) use sensor_settings::{SensorSettings, TransportModePanel};

mod image_viewer;
pub(crate) use image_viewer::ImageViewer;

pub(crate) mod screen_picker;
pub(crate) use screen_picker::{ScreenPicker, use_monitor_selection};

pub(crate) mod wizard;
