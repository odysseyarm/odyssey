#![expect(
    unused_qualifications,
    reason = "dioxus-rsx expands event attributes such as `onclick:` into paths the lint \
              flags at the user's span; they cannot be shortened in the markup"
)]

mod navbar;
pub(crate) use navbar::Navbar;

pub(crate) mod crosshair_manager;

pub(crate) mod firmware_update;
pub(crate) use firmware_update::{DeviceFirmwareUpdate, FirmwareUpdateManager, UpdatingDeviceRow};

pub(crate) mod sensor_settings;
pub(crate) use sensor_settings::{SensorSettings, TransportModePanel};

pub(crate) mod screen_picker;
pub(crate) use screen_picker::{ScreenPicker, use_monitor_selection};

pub(crate) mod wizard;
