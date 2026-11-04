//! The app's routed pages.

// dioxus-rsx 0.7 expands every event handler (`onclick: ...`) to the qualified
// path `dioxus_elements::events::onclick::...` spanned at the user's code, which
// rustc then reports as an unnecessary qualification. The markup cannot avoid it.
#![expect(
    unused_qualifications,
    reason = "false positive on rsx! event-handler expansion"
)]

mod accessories;
pub(crate) mod calibrate;
mod home;
pub(crate) mod pairing;
mod settings;
mod zero;
pub(crate) use accessories::Accessories;
pub(crate) use calibrate::Calibrate;
pub(crate) use calibrate::automatic::CalibrateAutomatic;
pub(crate) use home::Home;
pub(crate) use pairing::Pairing;
pub(crate) use settings::Settings;
pub(crate) use zero::Zero;

pub(crate) fn device_label(device: &odyssey_hub_common::device::Device) -> String {
    let addr = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        device.uuid[0],
        device.uuid[1],
        device.uuid[2],
        device.uuid[3],
        device.uuid[4],
        device.uuid[5]
    );
    let device_type = match device.product_id {
        0x520F => "ATS VM",
        0x5210 => "ATS Lite",
        0x5211 => "ATS Lite1",
        _ => "Unknown",
    };
    let is_dfu = device
        .capabilities
        .contains(odyssey_hub_common::device::DeviceCapabilities::DFU)
        && !device
            .capabilities
            .contains(odyssey_hub_common::device::DeviceCapabilities::CONTROL);
    let name = device.name().to_string();
    if is_dfu {
        format!("{device_type} [DFU] ({addr})")
    } else if name.is_empty() {
        format!("{device_type} ({addr})")
    } else {
        format!("{device_type}: {name} ({addr})")
    }
}
