mod accessories;
mod home;
pub mod pairing;
mod settings;
mod zero;
pub use accessories::Accessories;
pub use home::Home;
pub use pairing::Pairing;
pub use settings::Settings;
pub use zero::Zero;

pub fn device_label(device: &odyssey_hub_common::device::Device) -> String {
    let addr = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        device.uuid[0], device.uuid[1], device.uuid[2],
        device.uuid[3], device.uuid[4], device.uuid[5]
    );
    let device_type = match device.product_id {
        0x520F => "ATS VM",
        0x5210 => "ATS Lite",
        0x5211 => "ATS Lite1",
        _ => "Unknown",
    };
    let is_dfu = device.capabilities.contains(odyssey_hub_common::device::DeviceCapabilities::DFU)
        && !device.capabilities.contains(odyssey_hub_common::device::DeviceCapabilities::CONTROL);
    let name = device.name().to_string();
    if is_dfu {
        format!("{} [DFU] ({})", device_type, addr)
    } else if name.is_empty() {
        format!("{} ({})", device_type, addr)
    } else {
        format!("{}: {} ({})", device_type, name, addr)
    }
}
